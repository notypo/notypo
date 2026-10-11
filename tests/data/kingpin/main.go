// Build this opt-in fixture with `go build -o /tmp/notypo-kingpin-fixture .`
// from this directory, then set NOTYPO_TEST_KINGPIN_FIXTURE to that binary.
package main

import (
	"encoding/json"
	"os"
	"path/filepath"

	"github.com/alecthomas/kingpin/v2"
)

func marker(name string) {
	if dir := os.Getenv("NOTYPO_TEST_KINGPIN_MARKERS"); dir != "" {
		_ = os.WriteFile(filepath.Join(dir, name), []byte("called"), 0o600)
	}
}

func main() {
	app := kingpin.New("kingpin-fixture", "Completion boundary fixture")
	app.Flag("config", "Configuration file").String()
	app.Flag("log-file", "Default file opened by argument parsing").
		Default("default-value-file").OpenFile(os.O_CREATE|os.O_WRONLY, 0o600)
	app.PreAction(func(*kingpin.ParseContext) error {
		if dir := os.Getenv("NOTYPO_TEST_KINGPIN_MARKERS"); dir != "" {
			file, err := os.OpenFile(filepath.Join(dir, "queries"), os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0o600)
			if err != nil {
				return err
			}
			defer file.Close()
			_ = json.NewEncoder(file).Encode(os.Args[1:])
		}
		return nil
	})
	operation := func(*kingpin.ParseContext) error {
		marker("operation-marker")
		return nil
	}
	group := app.Command("snapshot", "Snapshot commands").Alias("s").Default()
	restore := group.Command("restore", "Restore an object").Alias("r").Default().Action(operation)
	restore.Flag("parallel", "Restore parallelism").Int()
	restore.Arg("object", "Object identifier").Required().HintAction(func() []string {
		marker("hint-marker")
		return []string{"live-resource"}
	}).String()
	list := group.Command("list", "List snapshots").Alias("ls").Action(operation)
	list.Flag("json", "JSON output").Bool()
	app.Command("inspect", "Inspect configuration").Action(operation)
	if dir := os.Getenv("NOTYPO_TEST_KINGPIN_MARKERS"); dir != "" {
		if _, err := os.Stat(filepath.Join(dir, "extension-enabled")); err == nil {
			app.Command("extension", "A command registered by a new extension").Action(operation)
		}
	}
	kingpin.MustParse(app.Parse(os.Args[1:]))
}
