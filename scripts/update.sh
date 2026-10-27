#!/usr/bin/env bash
# Rebuild and reinstall dredge on macOS or Linux.
#
#   ./scripts/update.sh            reinstall; running sessions are not interrupted
#   ./scripts/update.sh --pull     git pull --ff-only first
#   ./scripts/update.sh --stop     end every running `dredge` process first, then install
#
# Unlike Windows, Unix lets `cargo install` replace an exe that is running (it renames a new
# file over the old one), so no workaround is needed. Running sessions keep the old code until
# they end; new sessions use the new binary. --stop disconnects the dredge MCP server in every
# session that uses it, and those sessions must restart or reconnect it. Use it only when no
# session is mid-task.
set -euo pipefail

stop=0
pull=0
for arg in "$@"; do
    case "$arg" in
        --stop) stop=1 ;;
        --pull) pull=1 ;;
        -h | --help)
            sed -n '2,12p' "$0" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *)
            echo "unknown option: $arg (try --help)" >&2
            exit 2
            ;;
    esac
done

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export PATH="$HOME/.cargo/bin:$PATH"

if ! command -v cargo > /dev/null 2>&1; then
    echo "cargo not found. Install Rust first (see docs/INSTALL.md)." >&2
    exit 1
fi

if [ "$pull" -eq 1 ]; then
    git -C "$repo" pull --ff-only
fi

if [ "$stop" -eq 1 ]; then
    if pgrep -x dredge > /dev/null 2>&1; then
        echo "Stopping dredge processes..."
        pkill -x dredge || true
        sleep 0.5
    fi
fi

cd "$repo"
cargo install --path crates/dredge-cli

"$HOME/.cargo/bin/dredge" --version
if [ "$stop" -eq 1 ]; then
    echo "Restart the agent sessions (or reconnect their dredge MCP server) to use dredge again."
else
    echo "Running sessions keep the old version until they end; new sessions use the new one."
fi
