"""Measure the original Python engine with the same five preloaded rules."""

import json
import sys
from time import perf_counter_ns

from thefuck import corrector, utils
from thefuck.conf import settings
from thefuck.shells import shell
from thefuck.types import Command


def main():
    request = json.load(sys.stdin)
    fixture = request["fixture"]
    iterations = request["iterations"]
    settings.init()
    settings.rules = ["cd_parent", "mkdir_p", "git_not_command", "sudo", "no_command"]
    # Inject the same preloaded candidate list and history as the Rust Context.
    # Patch before loading rules, which import these functions by name.
    utils.get_all_executables = lambda: request["executables"]
    shell.get_history = lambda: ["git status"]
    rules = corrector.get_rules()
    assert len(rules) == 5

    def correct():
        command = Command(fixture["script"], fixture["output"])
        # This is get_corrected_commands's generator with discovery hoisted out
        # of the loop: both engines operate on already loaded rule functions.
        suggestions = (
            suggestion
            for rule in rules
            if rule.is_match(command)
            for suggestion in rule.get_corrected_commands(command)
        )
        return next(corrector.organize_commands(suggestions)).script

    for _ in range(10):
        assert correct() == fixture["expected"]
    times = []
    for _ in range(request["samples"]):
        start = perf_counter_ns()
        for _ in range(iterations):
            correct()
        times.append((perf_counter_ns() - start) / iterations)
    print(json.dumps({
        "ns_per_correction": times,
        "iterations": iterations,
        "rule_count": len(rules),
        "correction": fixture["expected"],
    }))


if __name__ == "__main__":
    main()
