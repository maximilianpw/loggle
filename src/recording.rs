//! Bounded, nonblocking submission; accepted lines are ordered and flushed on
//! close. Full queues fail explicitly (the caller decides fatal vs auxiliary).
use std::{
    fs::File,
    io::{self, BufWriter, Write},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use crate::page_log::{ActiveLogPageRegistration, PageLogRecorder};

pub(crate) enum Sink {
    Session(BufWriter<File>),
    Page(PageLogRecorder, ActiveLogPageRegistration),
}

impl Sink {
    fn record(&mut self, line: &str) -> io::Result<()> {
        match self {
            Self::Session(writer) => writeln!(writer, "{line}"),
            Self::Page(writer, _) => writer.record_line(line),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Session(writer) => writer.flush(),
            Self::Page(writer, _registration) => writer.flush(),
        }
    }
}

pub(crate) struct Recorder {
    tx: Option<mpsc::SyncSender<String>>,
    worker: Option<JoinHandle<io::Result<()>>>,
    error: Arc<Mutex<Option<String>>>,
    queued_bytes: Arc<AtomicUsize>,
}

impl Recorder {
    pub(crate) fn create(path: PathBuf) -> io::Result<Self> {
        Self::start(Sink::Session(BufWriter::new(File::create(path)?)))
    }

    pub(crate) fn start(mut sink: Sink) -> io::Result<Self> {
        let (tx, rx) = mpsc::sync_channel::<String>(1024);
        let error = Arc::new(Mutex::new(None));
        let worker_error = error.clone();
        let queued_bytes = Arc::new(AtomicUsize::new(0));
        let worker_bytes = queued_bytes.clone();
        let worker = thread::Builder::new()
            .name("loggle-writer".into())
            .spawn(move || {
                let result = (|| {
                    let mut flushed = Instant::now();
                    loop {
                        match rx.recv_timeout(Duration::from_millis(50)) {
                            Ok(line) => {
                                worker_bytes.fetch_sub(line.len(), Ordering::Relaxed);
                                sink.record(&line)?;
                            }
                            Err(mpsc::RecvTimeoutError::Timeout) => {}
                            Err(mpsc::RecvTimeoutError::Disconnected) => return sink.flush(),
                        }
                        if flushed.elapsed() >= Duration::from_millis(50) {
                            sink.flush()?;
                            flushed = Instant::now();
                        }
                    }
                })();
                if let Err(error) = &result {
                    *worker_error.lock().unwrap() = Some(error.to_string());
                }
                result
            })?;
        Ok(Self {
            tx: Some(tx),
            worker: Some(worker),
            error,
            queued_bytes,
        })
    }

    pub(crate) fn check(&self) -> io::Result<()> {
        match &*self.error.lock().unwrap() {
            Some(error) => Err(io::Error::other(error.clone())),
            None => Ok(()),
        }
    }

    pub(crate) fn record_line(&mut self, line: &str) -> io::Result<()> {
        self.check()?;
        let tx = self
            .tx
            .as_ref()
            .ok_or_else(|| io::Error::other("recording closed"))?;
        self.queued_bytes
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |bytes| {
                bytes
                    .checked_add(line.len())
                    .filter(|total| *total <= 8 * 1024 * 1024)
            })
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "recording byte budget full; output is incomplete",
                )
            })?;
        tx.try_send(line.to_owned()).map_err(|error| {
            self.queued_bytes.fetch_sub(line.len(), Ordering::Relaxed);
            match error {
                mpsc::TrySendError::Full(_) => io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "recording queue full; recording stopped, output is incomplete",
                ),
                mpsc::TrySendError::Disconnected(_) => io::Error::other("recording worker stopped"),
            }
        })
    }

    /// Close admission, drain accepted writes and join. This is a shutdown-only
    /// disk barrier, not an operation for the interactive loop. No fsync promise.
    pub(crate) fn finish(&mut self) -> io::Result<()> {
        self.tx.take();
        if let Some(worker) = self.worker.take() {
            worker
                .join()
                .map_err(|_| io::Error::other("recording worker panicked"))??;
        }
        self.check()
    }
}

// Dropping closes the queue but deliberately does not join: disabling an
// overloaded auxiliary writer must not block the UI. The worker owns its page
// registration until its accepted writes finish (or fail).

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_queue_fails_without_waiting_or_silently_dropping() {
        let (tx, rx) = mpsc::sync_channel(1);
        let mut recorder = Recorder {
            tx: Some(tx),
            worker: None,
            error: Arc::new(Mutex::new(None)),
            queued_bytes: Arc::new(AtomicUsize::new(0)),
        };
        recorder.record_line("accepted").unwrap();
        assert_eq!(
            recorder.record_line("rejected").unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        assert_eq!(rx.recv().unwrap(), "accepted");
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn finish_drains_ordered_writes_and_rejects_further_admission() {
        let path = std::env::temp_dir().join(format!("loggle-writer-{}.log", std::process::id()));
        let mut recorder = Recorder::create(path.clone()).unwrap();
        for index in 0..1000 {
            recorder.record_line(&index.to_string()).unwrap();
        }
        recorder.finish().unwrap();
        let lines = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            lines,
            (0..1000)
                .map(|index| format!("{index}\n"))
                .collect::<String>()
        );
        assert!(recorder.record_line("closed").is_err());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn byte_budget_bounds_a_stalled_writer_before_record_count_limit() {
        let (tx, _rx) = mpsc::sync_channel(1024);
        let mut recorder = Recorder {
            tx: Some(tx),
            worker: None,
            error: Arc::new(Mutex::new(None)),
            queued_bytes: Arc::new(AtomicUsize::new(0)),
        };
        let line = "x".repeat(64 * 1024);
        for _ in 0..128 {
            recorder.record_line(&line).unwrap();
        }
        assert_eq!(
            recorder.record_line(&line).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        assert_eq!(
            recorder.queued_bytes.load(Ordering::Relaxed),
            8 * 1024 * 1024
        );
    }

    #[test]
    fn disk_error_is_reported_even_after_input_stops() {
        let mut recorder = Recorder::create(PathBuf::from("/dev/full")).unwrap();
        recorder.record_line("failure").unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while recorder.check().is_ok() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(recorder.check().is_err());
        assert!(recorder.finish().is_err());
    }
}
