defmodule Loggle.CLI do
  alias Loggle.{Bridge, Lines, Screen, Store}

  @usage "Usage: loggle NAME='shell command' [NAME='shell command']\nExample: loggle api='mix phx.server' web='npm run dev'"

  def main(args \\ System.argv()) do
    case parse(args) do
      {:ok, commands} ->
        run(commands)

      :help ->
        IO.puts(@usage)

      {:error, reason} ->
        IO.puts(:stderr, reason <> "\n" <> @usage)
        System.halt(2)
    end
  end

  def parse(args) do
    commands = Enum.map(args, &String.split(&1, "=", parts: 2))

    cond do
      args == ["--help"] ->
        :help

      length(args) not in 1..2 ->
        {:error, "Provide one or two named commands."}

      not Enum.all?(commands, fn
        [name, cmd] ->
          Regex.match?(~r/^[a-zA-Z0-9_-]{1,12}$/, name) and cmd != "" and byte_size(cmd) <= 16384

        _ ->
          false
      end) ->
        {:error,
         "Names must be 1-12 letters/digits/_/-, with a nonempty shell command (max 16KiB)."}

      length(Enum.uniq_by(commands, &hd/1)) != length(commands) ->
        {:error, "Command names must be unique."}

      true ->
        {:ok, commands}
    end
  end

  def run(commands) do
    Process.flag(:trap_exit, true)
    tty = Bridge.open(["tty", System.get_env("LOGGLE_TTY", "/dev/tty")])
    Bridge.request(tty)

    receive do
      {^tty, {:data, <<?T, rows::16, cols::16, _::binary>>}} ->
        {:ok, device} = File.open("/dev/tty", [:write])

        try do
          IO.binwrite(device, "\e[?1049h\e[?25l\e[2J")
          start(commands, tty, device, rows, cols)
        after
          # Closing every owned port also covers partial startup and exceptions.
          for port <- Port.list(),
              Port.info(port, :connected) == {:connected, self()},
              do: Bridge.close(port)

          IO.binwrite(device, "\e[0m\e[?25h\e[?1049l")
          File.close(device)
          Process.sleep(500)
        end

      {^tty, {:exit_status, _}} ->
        raise "Loggle needs a controlling terminal (/dev/tty)."
    after
      2000 ->
        Bridge.close(tty)
        raise "Terminal setup timed out"
    end
  end

  defp start(commands, tty, device, rows, cols) do
    ports =
      Map.new(commands, fn [name, command] ->
        port = Bridge.open(["command", command])
        Bridge.request(port)

        {port,
         %{name: name, status: "running", pending: true, out: Lines.new(), err: Lines.new()}}
      end)

    loop(%{
      commands: ports,
      tty: tty,
      tty_pending: false,
      device: device,
      rows: min(rows, 100),
      cols: min(cols, 240),
      store: Store.new(),
      follow: true,
      anchor: 0,
      filter: "",
      source: nil,
      edit: false,
      draft: "",
      quit: false
    })
  end

  defp loop(state) do
    IO.binwrite(state.device, Screen.render(state))
    if !state.tty_pending, do: Bridge.request(state.tty)
    state = %{state | tty_pending: true}
    # Fixed cadence, then drain only the bounded replies already requested.
    Process.sleep(50)
    state = drain(state)

    if !state.quit do
      commands =
        Map.new(state.commands, fn {port, c} ->
          if !c.pending and c.status == "running" do
            Bridge.request(port)
            {port, %{c | pending: true}}
          else
            {port, c}
          end
        end)

      loop(%{state | commands: commands})
    end
  end

  defp drain(state) do
    receive do
      {port, {:data, <<?T, rows::16, cols::16, keys::binary>>}} when port == state.tty ->
        state = Enum.reduce(:binary.bin_to_list(keys), state, &Screen.key(&2, &1))
        drain(%{state | rows: min(rows, 100), cols: min(cols, 240), tty_pending: false})

      {port, {:data, <<kind, bytes::binary>>}} when is_map_key(state.commands, port) ->
        c = state.commands[port]

        {store, c} =
          case kind do
            k when k in [?O, ?R] ->
              field = if k == ?O, do: :out, else: :err
              {lines, framing} = Lines.feed(Map.fetch!(c, field), bytes)
              {append(state.store, c.name, field, lines), Map.put(c, field, framing)}

            ?E ->
              store = append(state.store, c.name, :out, Lines.flush(c.out))
              store = append(store, c.name, :err, Lines.flush(c.err))
              {store, %{c | status: "exit #{bytes}"}}
          end

        drain(%{
          state
          | store: store,
            commands: Map.put(state.commands, port, %{c | pending: false})
        })

      {port, {:exit_status, code}} when is_map_key(state.commands, port) ->
        c = state.commands[port]
        c = if c.status == "running", do: %{c | status: "bridge failed #{code}"}, else: c
        drain(%{state | commands: Map.put(state.commands, port, c)})

      {port, {:exit_status, _}} when port == state.tty ->
        %{state | quit: true}

      {:EXIT, _, :normal} ->
        drain(state)

      {:EXIT, _, reason} ->
        raise "Port failed: #{inspect(reason)}"
    after
      0 -> state
    end
  end

  defp append(store, name, stream, lines),
    do: Enum.reduce(lines, store, &Store.append(&2, name, stream, &1))
end
