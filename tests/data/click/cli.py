"""Real Click 8 fixture; installed_clis copies it beside a console script."""

import json
import os
import sys
from pathlib import Path

import click
from click.shell_completion import CompletionItem

ROOT = Path(os.environ["NOTYPO_TEST_CLICK_MARKERS"])
with (ROOT / "queries").open("a") as stream:
    stream.write(json.dumps({
        "args": sys.argv[1:],
        "instruction": os.environ.get("_CLICK_FIXTURE_COMPLETE"),
        "words": os.environ.get("COMP_WORDS"),
        "index": os.environ.get("COMP_CWORD"),
    }) + "\n")


def operation():
    (ROOT / "operation-marker").write_text("executed")


def parameter(ctx, param, value):
    (ROOT / "parameter-marker").write_text("converted during completion")
    return value


def teams(ctx, param, incomplete):
    (ROOT / "resource-marker").write_text("local callback")
    return [CompletionItem(value, help="Current local team") for value in [
        "team one",
        "it's;touch operation-marker",
        "$(touch operation-marker)",
        "team 😀 / western",
        "team:one",
        "team\\:two",
    ] if value.startswith(incomplete)]


@click.group()
@click.option("--profile", callback=parameter)
def cli(profile):
    """A fixture with groups, choices, and local resource callbacks."""
    operation()


@cli.group()
def deploy():
    """Manage deployments."""
    operation()


@deploy.command()
@click.option("--format", type=click.Choice(["json", "yaml"]))
@click.option("--team", shell_complete=teams)
@click.option("--label", type=click.Choice(["report package", "summary"]))
@click.option("--file", type=click.Path())
@click.option("--watch", is_flag=True)
@click.argument("target", type=click.Choice(["prod", "staging"]), required=False)
def status(**kwargs):
    """Show deployment status."""
    operation()


if (ROOT / "extension-enabled").exists():
    @cli.command()
    def extension():
        """An extension installed between requests."""
        operation()
