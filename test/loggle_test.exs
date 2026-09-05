defmodule LoggleTest do
  use ExUnit.Case
  alias Loggle.{Bridge, Lines, Store, Screen}

  test "physical streams frame independently, including split CRLF and final partial" do
    {[], out} = Lines.feed(Lines.new(), "one\r")
    {["error"], err} = Lines.feed(Lines.new(), "error\n")
    {["one", "two"], out} = Lines.feed(out, "\ntwo\nlast")
    assert Lines.flush(out) == ["last"]
    assert Lines.flush(err) == []
  end

  test "huge unterminated lines stay bounded and discard through the next newline" do
    pending =
      Enum.reduce(1..1000, Lines.new(), fn _, state ->
        {[], state} = Lines.feed(state, :binary.copy("x", 4096))
        assert byte_size(elem(state, 0)) <= 4096
        state
      end)

    {[line, "next"], state} = Lines.feed(pending, "tail\nnext\n")
    assert line == :binary.copy("x", 4096) <> " [truncated]"
    assert state == Lines.new()
  end

  test "retention has both count and byte bounds across repeated turnover" do
    store =
      Enum.reduce(1..20_000, Store.new(), fn _, store ->
        Store.append(store, "api", :out, "INFO hi")
      end)

    assert Store.count(store) == 2000
    assert store.evicted == 18_000

    store =
      Enum.reduce(1..5000, store, fn _, store ->
        Store.append(store, "web", :err, :binary.copy("x", 4096))
      end)

    assert store.bytes <= 8 * 1024 * 1024
    assert Store.count(store) < 2000
    assert store.next == 25_001
  end

  test "plain and JSON parse without losing named source or pipe identity" do
    store =
      Store.new()
      |> Store.append("api", :out, ~s({"level":"warn","message":"retry"}))
      |> Store.append("api", :err, "ERROR failure")
      |> Store.append("web", :out, <<255, 27>>)

    assert [
             {1, "api", :out, "warn", "retry", _},
             {2, "api", :err, "error", "ERROR failure", _},
             {3, "web", :out, "log", _, _}
           ] = Store.rows(store)

    assert Screen.safe(<<27, 10, 255, 65>>) == "???A"
  end

  test "named command validation is bounded and rejects duplicates before launch" do
    assert {:ok, [["api", "echo a=b"]]} = Loggle.CLI.parse(["api=echo a=b"])

    for args <- [[], ["bad"], ["a="], ["a=x", "a=y"], ["a=x", "b=y", "c=z"], ["bad name=x"]] do
      assert {:error, _} = Loggle.CLI.parse(args)
    end
  end

  test "navigation freezes a position, filters apply on Enter, Ctrl-C always quits" do
    state = %{
      follow: true,
      anchor: 0,
      store: %{next: 50},
      edit: false,
      draft: "",
      filter: "",
      quit: false
    }

    assert %{anchor: 49, follow: false} = Screen.key(state, ?j)
    state = Screen.key(state, ?k)
    assert state.anchor == 48
    assert Screen.key(state, ?j).anchor == 49
    state = state |> Screen.key(?/) |> Screen.key(?x)
    assert state.filter == ""
    assert state.draft == "x"
    assert Screen.key(state, 13).filter == "x"
    assert Screen.key(state, 3).quit
  end

  test "ports give no unsolicited flood output and only one bounded reply per credit" do
    port = Bridge.open(["command", "yes flood"])

    try do
      refute_receive {^port, {:data, _}}, 150
      Bridge.request(port)
      assert_receive {^port, {:data, <<?O, bytes::binary>>}}, 1000
      assert byte_size(bytes) <= 4096
      refute_receive {^port, {:data, _}}, 150
      assert {:message_queue_len, 0} = Process.info(self(), :message_queue_len)
    after
      Bridge.close(port)
    end
  end

  test "separate pipes, EOF, partial output and nonzero status survive the bridge" do
    port = Bridge.open(["command", "printf out; printf err >&2; exit 7"])
    {out, err, code} = collect(port, "", "")
    assert {out, err, code} == {"out", "err", "7"}
  end

  defp collect(port, out, err) do
    Bridge.request(port)

    receive do
      {^port, {:data, <<?O, bytes::binary>>}} -> collect(port, out <> bytes, err)
      {^port, {:data, <<?R, bytes::binary>>}} -> collect(port, out, err <> bytes)
      {^port, {:data, <<?E, code::binary>>}} -> {out, err, code}
    after
      3000 -> flunk("bridge timed out")
    end
  end
end
