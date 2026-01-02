# Orchestration Scripts

These scripts help monitor and coordinate the parallel development of Rust rewrite iterations.

## Scripts

### `check-iteration-status.sh`
Check the current status of all iteration PRs, including CI status.

```bash
./scripts/check-iteration-status.sh
```

### `watch-all-iterations.sh`
Continuously monitor GitHub Actions for all iteration PRs. Updates every 30 seconds.

```bash
./scripts/watch-all-iterations.sh
```

Press `Ctrl+C` to stop watching.

### `check-conflicts.sh`
Test merge all iteration branches to detect conflicts early.

```bash
./scripts/check-conflicts.sh
```

This creates a temporary clone and attempts to merge all branches. Reports any conflicts found.

## Requirements

- `gh` (GitHub CLI) authenticated with appropriate permissions
- `git` command-line tools
- Bash 4.0+

## Usage Example

```bash
# Quick status check
./scripts/check-iteration-status.sh

# Start monitoring (runs continuously)
./scripts/watch-all-iterations.sh

# Before integration, check for conflicts
./scripts/check-conflicts.sh
if [ $? -eq 0 ]; then
    echo "Ready for integration!"
else
    echo "Conflicts detected - resolve before integration"
fi
```

## Notes

- All scripts use the GitHub CLI (`gh`) to fetch real-time data
- Scripts are designed to work from the repository root
- Exit codes: 0 = success, 1 = errors or conflicts detected
