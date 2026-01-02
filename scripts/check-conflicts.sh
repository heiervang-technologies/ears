#!/usr/bin/env bash
# Check for merge conflicts between iteration branches

set -euo pipefail

REPO="heiervang-technologies/ears"
ITERATIONS=(17 18 19 20 21 23 24)

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "Checking for Merge Conflicts"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

# Create temporary directory for testing merges
tmpdir=$(mktemp -d)
cd "$tmpdir"

echo "📥 Cloning repository..."
git clone --quiet "https://github.com/$REPO.git" repo
cd repo

# Get default branch
default_branch=$(git symbolic-ref refs/remotes/origin/HEAD | sed 's@^refs/remotes/origin/@@')

echo "✅ Cloned to $tmpdir/repo"
echo "🌿 Default branch: $default_branch"
echo ""

# Collect all PR branches
declare -a branches
for issue in "${ITERATIONS[@]}"; do
    # Find PRs for this issue
    pr_branch=$(gh pr list --repo "$REPO" --search "fixes #$issue OR closes #$issue" --json headRefName --jq '.[0].headRefName // empty')

    if [[ -n "$pr_branch" ]]; then
        branches+=("$pr_branch")
        echo "Found branch for issue #$issue: $pr_branch"
    fi
done

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "Testing Merges"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

conflicts_found=0

# Try merging all branches into a test branch
git checkout -b integration-test "$default_branch"

for branch in "${branches[@]}"; do
    echo "🔀 Attempting to merge: $branch"

    git fetch origin "$branch"

    if git merge --no-commit --no-ff "origin/$branch" &>/dev/null; then
        echo "   ✅ No conflicts"
        git merge --abort &>/dev/null || true
    else
        echo "   ⚠️  CONFLICTS DETECTED!"
        echo "   Files with conflicts:"
        git diff --name-only --diff-filter=U | while read -r file; do
            echo "      - $file"
        done
        conflicts_found=$((conflicts_found + 1))
        git merge --abort &>/dev/null || true
    fi

    echo ""
done

# Cleanup
cd /
rm -rf "$tmpdir"

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "📊 Summary"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""
echo "Branches checked: ${#branches[@]}"
echo "Conflicts found: $conflicts_found"
echo ""

if [[ $conflicts_found -eq 0 ]]; then
    echo "✅ All branches can be merged without conflicts!"
    exit 0
else
    echo "⚠️  Manual conflict resolution required"
    exit 1
fi
