defmodule Loggle.Store do
  @moduledoc "UI-owned ETS tail; both record count and accounted payload are bounded."
  def new do
    %{table: :ets.new(__MODULE__, [:ordered_set, :private]), next: 1, bytes: 0, evicted: 0}
  end

  def append(store, name, stream, raw) do
    {level, message} = parse(raw)
    row = {store.next, name, stream, level, message, raw}
    size = :erlang.external_size(row)
    :ets.insert(store.table, {store.next, row, size})
    trim(%{store | next: store.next + 1, bytes: store.bytes + size})
  end

  def rows(store), do: Enum.map(:ets.tab2list(store.table), fn {_, row, _} -> row end)
  def count(store), do: :ets.info(store.table, :size)

  defp trim(store) do
    if count(store) > 2000 or store.bytes > 8 * 1024 * 1024 do
      key = :ets.first(store.table)
      [{_, _, size}] = :ets.lookup(store.table, key)
      :ets.delete(store.table, key)
      trim(%{store | bytes: store.bytes - size, evicted: store.evicted + 1})
    else
      store
    end
  end

  defp parse(raw) do
    case Jason.decode(raw) do
      {:ok, map} when is_map(map) ->
        level = map["level"] || map["severity"] || "log"
        message = map["message"] || map["msg"] || raw

        {if(is_binary(level), do: level, else: "log"),
         if(is_binary(message), do: message, else: raw)}

      _ ->
        level =
          case Regex.run(~r/\b(ERROR|WARN|INFO|DEBUG|TRACE|FATAL)\b/i, raw) do
            [_, level] -> String.downcase(level)
            _ -> "log"
          end

        {level, raw}
    end
  end
end
