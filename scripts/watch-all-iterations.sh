#!/usr/bin/env bash
# Watch GitHub Actions for all iteration PRs

set -euo pipefail

REPO="heiervang-technologies/ears"
ITERATIONS=(17 18 19 20 21 23 24)

echo "🔍 Watching GitHub Actions for all rust-rewrite iterations..."
echo "Press Ctrl+C to stop"
echo ""

while true; do
    clear
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "Rust Rewrite - Live Status Monitor"
    echo "Updated: $(date '+%Y-%m-%d %H:%M:%S')"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo ""

    for issue in "${ITERATIONS[@]}"; do
        # Get issue title
        title=$(gh issue view "$issue" --repo "$REPO" --json title --jq '.title' 2>/dev/null || echo "Unknown")

        # Check for associated PRs
        pr_num=$(gh pr list --repo "$REPO" --search "fixes #$issue OR closes #$issue" --json number --jq '.[0].number // empty' 2>/dev/null)

        if [[ -z "$pr_num" ]]; then
            echo "⏳ Issue #$issue: No PR yet"
            echo "   $title"
        else
            pr_state=$(gh pr view "$pr_num" --repo "$REPO" --json state --jq '.state')

            echo "📋 Issue #$issue → PR #$pr_num [$pr_state]"
            echo "   $title"

            # Get CI status
            if gh pr checks "$pr_num" --repo "$REPO" &>/dev/null; then
                gh pr checks "$pr_num" --repo "$REPO" 2>/dev/null | while read -r line; do
                    echo "   $line"
                done
            else
                echo "   ⚪ No CI checks yet"
            fi
        fi

        echo ""
    done

    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

    # Sleep for 30 seconds before next refresh
    sleep 30
done
