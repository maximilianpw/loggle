/* Bounded demand protocol: one-byte requests; packet-2 replies <= 4097 bytes.
 * This process, not BEAM, owns Unix pipes, the session, and cleanup on EOF.
 * No log parsing or UI policy belongs here. */
#define _POSIX_C_SOURCE 200809L
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <termios.h>
#include <time.h>
#include <unistd.h>

static volatile sig_atomic_t stopped;
static void stop(int sig) { (void)sig; stopped = 1; }
static void nap(void) { struct timespec t = {0, 10000000}; nanosleep(&t, NULL); }
static int write_all(const void *data, size_t n) {
    const char *p = data;
    while (n) {
        ssize_t k = write(1, p, n);
        if (k < 0 && errno == EINTR && !stopped) continue;
        if (k <= 0) return -1;
        p += k; n -= (size_t)k;
    }
    return 0;
}
static int packet(char type, const void *data, size_t n) {
    unsigned char h[3] = {(unsigned char)((n+1)>>8), (unsigned char)(n+1), (unsigned char)type};
    return write_all(h, 3) || write_all(data, n);
}
static int request(void) {
    /* Elixir uses packet: 2 in both directions. Only a single-byte body is valid. */
    unsigned char b[3]; size_t n = 0;
    while (n < 3 && !stopped) {
        ssize_t k = read(0, b+n, 3-n);
        if (k < 0 && errno == EINTR) continue;
        if (k <= 0) return 0;
        n += (size_t)k;
    }
    return n == 3 && b[0] == 0 && b[1] == 1 && b[2] == 'r';
}
static int terminal(const char *path) {
    int fd = open(path, O_RDWR | O_NONBLOCK);
    struct termios saved, raw;
    if (fd < 0 || tcgetattr(fd, &saved)) return 1;
    raw = saved;
    raw.c_lflag &= ~(ICANON | ECHO | ISIG | IEXTEN);
    raw.c_iflag &= ~(IXON | ICRNL);
    raw.c_cc[VMIN] = 0; raw.c_cc[VTIME] = 0;
    if (tcsetattr(fd, TCSANOW, &raw)) { close(fd); return 1; }
    while (request()) {
        struct winsize w = {0}; unsigned char b[68];
        ioctl(fd, TIOCGWINSZ, &w);
        unsigned rows = w.ws_row ? w.ws_row : 24, cols = w.ws_col ? w.ws_col : 80;
        b[0] = rows>>8; b[1] = rows; b[2] = cols>>8; b[3] = cols;
        ssize_t n = read(fd, b+4, 64);
        if (packet('T', b, 4 + (n > 0 ? (size_t)n : 0))) break;
    }
    tcsetattr(fd, TCSANOW, &saved);
    /* Also restore the screen if the VM died without its after block. */
    const char *reset = "\033[0m\033[?25h\033[?1049l";
    (void)write(fd, reset, strlen(reset));
    close(fd); return 0;
}
static int cleanup(pid_t pid) {
    /* Keep the leader unreaped until the last signal, preventing PGID reuse. */
    int status = 0;
    kill(-pid, SIGINT);
    for (int i = 0; i < 20; i++) nap();
    kill(-pid, SIGTERM);
    for (int i = 0; i < 20; i++) nap();
    kill(-pid, SIGKILL);
    while (waitpid(pid, &status, 0) < 0 && errno == EINTR) {}
    return status;
}
static int command(const char *cmd) {
    int out[2], err[2], ready[2];
    if (pipe(out) || pipe(err) || pipe(ready)) return 1;
    pid_t pid = fork();
    if (pid < 0) return 1;
    if (pid == 0) {
        close(ready[0]);
        if (setsid() < 0) _exit(126);
        int null = open("/dev/null", O_RDONLY);
        if (null < 0 || dup2(null, 0) < 0 || dup2(out[1], 1) < 0 || dup2(err[1], 2) < 0) _exit(126);
        close(null); close(out[0]); close(out[1]); close(err[0]); close(err[1]);
        signal(SIGINT, SIG_DFL); signal(SIGTERM, SIG_DFL); signal(SIGPIPE, SIG_DFL);
        if (write(ready[1], "!", 1) != 1) _exit(126);
        close(ready[1]);
        execl("/bin/sh", "sh", "-c", cmd, (char *)NULL);
        _exit(127);
    }
    close(out[1]); close(err[1]); close(ready[1]);
    char ack; ssize_t acked;
    do { acked = read(ready[0], &ack, 1); } while (acked < 0 && errno == EINTR);
    close(ready[0]);
    if (acked != 1) { waitpid(pid, NULL, 0); return 1; }
    struct pollfd f[3] = {{0, POLLIN, 0}, {out[0], POLLIN, 0}, {err[0], POLLIN, 0}};
    int credit = 0, status = 0, turn = 0, cleaned = 0;
    while (!stopped) {
        siginfo_t info = {0};
        if (!cleaned && waitid(P_PID, (id_t)pid, &info, WEXITED | WNOHANG | WNOWAIT) == 0 && info.si_pid == pid) {
            status = cleanup(pid); cleaned = 1;
        }
        if (credit && f[1].fd < 0 && f[2].fd < 0 && cleaned) {
            char b[32]; int code = WIFEXITED(status) ? WEXITSTATUS(status) : 128 + WTERMSIG(status);
            int n = snprintf(b, sizeof b, "%d", code); packet('E', b, (size_t)n); break;
        }
        f[1].events = f[2].events = credit ? POLLIN : 0;
        /* Do not poll output fds without credit: POLLHUP would busy-loop. */
        struct pollfd p[3] = {f[0], f[1], f[2]};
        if (!credit) p[1].fd = p[2].fd = -1;
        int n = poll(p, 3, 50);
        if (n < 0) { if (errno == EINTR) continue; break; }
        if (p[0].revents & (POLLIN | POLLHUP | POLLERR)) {
            if (!request()) break;
            credit = 1;
        }
        for (int j = 0; j < 2 && credit; j++) {
            int i = 1 + ((turn + j) % 2);
            if (p[i].revents & (POLLIN | POLLHUP | POLLERR)) {
                char b[4096]; ssize_t k = read(f[i].fd, b, sizeof b);
                if (k > 0) {
                    if (packet(i == 1 ? 'O' : 'R', b, (size_t)k)) { stopped = 1; break; }
                    credit = 0; turn = i % 2;
                } else if (k == 0 || (errno != EINTR && errno != EAGAIN)) {
                    close(f[i].fd); f[i].fd = -1;
                }
            }
        }
    }
    if (!cleaned) cleanup(pid);
    if (f[1].fd >= 0) close(f[1].fd);
    if (f[2].fd >= 0) close(f[2].fd);
    return 0;
}
int main(int argc, char **argv) {
    signal(SIGPIPE, SIG_IGN);
    struct sigaction sa = {0}; sa.sa_handler = stop; sigemptyset(&sa.sa_mask);
    sigaction(SIGTERM, &sa, NULL); sigaction(SIGINT, &sa, NULL); sigaction(SIGHUP, &sa, NULL);
    if (argc == 3 && strcmp(argv[1], "tty") == 0) return terminal(argv[2]);
    if (argc == 3 && strcmp(argv[1], "command") == 0) return command(argv[2]);
    return 2;
}
