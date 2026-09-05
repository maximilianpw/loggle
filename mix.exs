defmodule Loggle.MixProject do
  use Mix.Project

  def project do
    [
      app: :loggle,
      version: "0.2.0-dev",
      elixir: "~> 1.14",
      compilers: [:native] ++ Mix.compilers(),
      deps: [{:jason, "~> 1.4"}],
      releases: [loggle: [include_erts: true, steps: [:assemble, &package_cli/1, :tar]]]
    ]
  end

  def application, do: [extra_applications: [:logger]]

  defp package_cli(release) do
    File.rename!(
      Path.join(release.path, "bin/loggle"),
      Path.join(release.path, "bin/loggle_runtime")
    )

    File.cp!("bin/loggle", Path.join(release.path, "bin/loggle"))
    release
  end
end

defmodule Mix.Tasks.Compile.Native do
  use Mix.Task.Compiler

  def run(_) do
    File.mkdir_p!("priv")

    {output, status} =
      System.cmd(
        "cc",
        ["-std=c11", "-O2", "-Wall", "-Wextra", "-Werror", "c_src/bridge.c", "-o", "priv/bridge"],
        stderr_to_stdout: true
      )

    if status != 0, do: Mix.raise(output)
    {:ok, []}
  end
end
