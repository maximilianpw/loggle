defmodule Loggle.Screen do
  @moduledoc "Small deterministic viewport. Terminal content is reduced to safe ASCII cells."
  def safe(text) do
    for <<byte <- text>>, into: "" do
      if byte >= 32 and byte <= 126, do: <<byte>>, else: "?"
    end
  end

  def render(state) do
    height = max(1, state.rows - 6)

    rows =
      Loggle.Store.rows(state.store)
      |> Enum.filter(fn {_, name, _, _, message, raw} ->
        (state.source == nil or state.source == name) and
          (String.contains?(raw, state.filter) or String.contains?(message, state.filter))
      end)
      |> Enum.filter(fn {id, _, _, _, _, _} -> state.follow or id <= state.anchor end)
      |> Enum.take(-height)

    status = Enum.map_join(state.commands, "  |  ", fn {_, c} -> "#{c.name}: #{c.status}" end)
    mode = if state.follow, do: "FOLLOW", else: "PAUSED"

    header =
      " LOGGLE / #{mode}   retained #{Loggle.Store.count(state.store)}   evicted #{state.store.evicted}"

    body =
      Enum.map(rows, fn {id, name, stream, level, msg, _} ->
        " #{String.pad_leading(to_string(id), 6)} #{String.pad_trailing(name, 12)} #{stream} #{String.pad_trailing(level, 5)} #{msg}"
      end)

    prompt =
      if state.edit,
        do: " /#{state.draft}_  Enter apply / Esc cancel",
        else: " text=#{state.filter}  source=#{state.source || "all"}"

    lines =
      [header, " #{status}", "     #  SOURCE       PIPE LEVEL MESSAGE"] ++
        body ++
        List.duplicate("", height - length(body)) ++
        [
          prompt,
          " q/Ctrl-C quit  p pause  j/k scroll  G follow  s source  / text  c clear",
          " 4KiB reads/lines | 2k rows / 8MiB tail | backpressure, no restart"
        ]

    [
      "\e[H",
      Enum.map_join(
        Enum.take(lines, state.rows),
        "\r\n",
        &(safe(&1) |> String.slice(0, max(1, state.cols - 1)) |> Kernel.<>("\e[K"))
      )
    ]
  end

  def key(state, 3), do: %{state | quit: true}

  def key(%{edit: true} = state, key) do
    case key do
      27 ->
        %{state | edit: false}

      k when k in [10, 13] ->
        %{state | edit: false, filter: state.draft}

      k when k in [8, 127] ->
        %{state | draft: String.slice(state.draft, 0, max(0, byte_size(state.draft) - 1))}

      k when k >= 32 and k <= 126 ->
        %{state | draft: String.slice(state.draft <> <<k>>, 0, 128)}

      _ ->
        state
    end
  end

  def key(state, key) do
    case key do
      ?q ->
        %{state | quit: true}

      ?p ->
        %{state | follow: !state.follow, anchor: state.store.next - 1}

      32 ->
        key(state, ?p)

      ?G ->
        %{state | follow: true}

      ?k ->
        %{
          state
          | follow: false,
            anchor: max(1, if(state.follow, do: state.store.next - 1, else: state.anchor) - 1)
        }

      ?j ->
        %{
          state
          | follow: false,
            anchor:
              min(
                state.store.next - 1,
                if(state.follow, do: state.store.next - 1, else: state.anchor + 1)
              )
        }

      ?/ ->
        %{state | edit: true, draft: state.filter}

      ?c ->
        %{state | filter: "", source: nil}

      ?s ->
        names = [nil | Enum.map(state.commands, fn {_, c} -> c.name end)]
        i = Enum.find_index(names, &(&1 == state.source)) || 0
        %{state | source: Enum.at(names, rem(i + 1, length(names)))}

      _ ->
        state
    end
  end
end
