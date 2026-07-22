# tic-tac-toe — justfile
# Design phase: recipes are placeholders until M1 scaffolds the app.

default:
    @just --list

# Full validation (fmt, lint, build, test, doc) — wired at M1
check:
    @echo "design phase — no code yet; see docs/design/tic-tac-toe-v1.md"

# Render the design doc tree
docs:
    @find docs -name '*.md' | sort
