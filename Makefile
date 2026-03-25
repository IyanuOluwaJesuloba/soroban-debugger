# Soroban Debugger — developer convenience targets
#
# Targets:
#   regen-man   Regenerate all man pages from current CLI source
#   check-man   Verify committed man pages match generated output (used in CI)

.PHONY: regen-man check-man

# Regenerate all man pages from current CLI source.
# Run after any CLI flag, subcommand, or help text change, then commit the .1 files.
regen-man:
	@echo "Regenerating man pages..."
	cargo build --quiet
	@echo "Man pages updated in man/man1/ — remember to commit the .1 files."

# Verify committed man pages match generated output.
# Exits non-zero with a diff if drift is detected.
check-man:
	@bash scripts/check_manpages.sh
