package root

import (
	"github.com/docker/cli/cli-plugins/plugin"
	"github.com/spf13/cobra"

	"github.com/docker/docker-agent/pkg/cli"
	"github.com/docker/docker-agent/pkg/telemetry"
	"github.com/docker/docker-agent/pkg/version"
	"github.com/docker/docker-agent/pkg/version/check"
)

func newVersionCmd() *cobra.Command {
	return &cobra.Command{
		Use:   "version",
		Short: "Print the version information",
		Long:  "Display the version and commit hash",
		Args:  cobra.NoArgs,
		Run:   runVersionCommand,
	}
}

func runVersionCommand(cmd *cobra.Command, args []string) {
	telemetry.TrackCommand(cmd.Context(), "version", args)

	out := cli.NewPrinter(cmd.OutOrStdout())

	commandName := "docker-agent"
	if cmd.Parent() != nil {
		commandName = cmd.Parent().Name()
	}
	if !plugin.RunningStandalone() {
		commandName = "docker " + commandName
	}
	out.Printf("%s version %s\n", commandName, version.Version)
	out.Printf("Commit: %s\n", version.Commit)

	// Best-effort upgrade hint. Failures (offline, rate-limited, …) are
	// silently ignored — the user simply does not see the line.
	if res := check.Latest(cmd.Context(), version.Version); res.UpgradeAvailable() {
		out.Printf("\nA newer version is available: %s\nRelease notes: https://github.com/docker/docker-agent/releases/tag/%s\n",
			res.Latest, res.Latest)
	}
}
