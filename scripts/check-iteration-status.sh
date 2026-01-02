#!/usr/bin/env bash
# Check the status of all rust-rewrite iteration PRs

set -euo pipefail

REPO="heiervang-technologies/ears"
ITERATIONS=(17 18 19 20 21 23 24)

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "Rust Rewrite Iteration Status"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

for issue in "${ITERATIONS[@]}"; do
    echo "📋 Issue #$issue"

    # Get issue title
    title=$(gh issue view "$issue" --repo "$REPO" --json title --jq '.title')
    echo "   Title: $title"

    # Check for associated PRs
    prs=$(gh pr list --repo "$REPO" --search "fixes #$issue OR closes #$issue" --json number,state,title,url --jq '.[] | "\(.number)|\(.state)|\(.title)|\(.url)"')

    if [[ -z "$prs" ]]; then
        echo "   Status: ⏳ No PR yet"
    else
        while IFS='|' read -r pr_num pr_state pr_title pr_url; do
            echo "   PR #$pr_num: $pr_state"
            echo "   URL: $pr_url"

            # Check CI status if PR exists
            if gh pr checks "$pr_num" --repo "$REPO" &>/dev/null; then
                echo "   CI Status:"
                gh pr checks "$pr_num" --repo "$REPO" | head -n 5
            fi
        done <<< "$prs"
    fi

    echo ""
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo ""
done

# Summary
echo "📊 Summary:"
total_prs=$(gh pr list --repo "$REPO" --search "rust-rewrite" --json number --jq '. | length')
echo "   Total PRs created: $total_prs / ${#ITERATIONS[@]}"

open_prs=$(gh pr list --repo "$REPO" --search "rust-rewrite is:open" --json number --jq '. | length')
echo "   Open PRs: $open_prs"

merged_prs=$(gh pr list --repo "$REPO" --search "rust-rewrite is:merged" --json number --jq '. | length')
echo "   Merged PRs: $merged_prs"

echo ""
echo "✅ Status check complete"
