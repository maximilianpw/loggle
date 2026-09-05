defmodule Loggle.Bridge do
  @moduledoc "Demand-driven native ports. At most one 4096-byte read is outstanding per command."
  def open(args) do
    Port.open(
      {:spawn_executable, :filename.join(:code.priv_dir(:loggle), ~c"bridge")},
      [:binary, :exit_status, {:packet, 2}, {:args, args}]
    )
  end

  def request(port), do: Port.command(port, "r")

  def close(port) do
    if Port.info(port), do: Port.close(port)
  end
end
