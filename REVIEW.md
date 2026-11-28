# REVIEW.md

@CODING_STANDARDS.md

This file holds this repository's review policy. Read `CODING_STANDARDS.md`
(included above) for the defect classes every change is checked against. State
each finding's evidence — file, line, and broken behavior — and cite the rule
it violates when one applies.

Check clean-room MIT licensing compliance (`deny.toml`), domain/port/adapter
layer boundaries (`scripts/check_layer_deps.py`), and observable contract tests
for every change.
