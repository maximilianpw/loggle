defmodule Loggle.Lines do
  @moduledoc "Bounded physical-line framing; oversized tails are discarded through newline."
  @limit 4096
  def new, do: {<<>>, false}

  def feed(state, bytes) do
    [first | rest] = :binary.split(bytes, "\n", [:global])
    {pending, clipped} = append(state, first)

    case rest do
      [] ->
        {[], {pending, clipped}}

      _ ->
        {complete, [last]} = Enum.split(rest, -1)
        lines = [finish({pending, clipped}) | Enum.map(complete, &finish(append(new(), &1)))]
        {lines, append(new(), last)}
    end
  end

  def flush({"", false}), do: []
  def flush(state), do: [finish(state)]

  defp append({pending, clipped}, bytes) do
    size = min(byte_size(bytes), @limit - byte_size(pending))
    {pending <> binary_part(bytes, 0, size), clipped or size < byte_size(bytes)}
  end

  defp finish({text, clipped}),
    do: String.trim_trailing(text, "\r") <> if(clipped, do: " [truncated]", else: "")
end
