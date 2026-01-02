#!/usr/bin/env bash
#
# Test runner for ears
# Runs all BATS tests with proper setup
#

set -euo pipefail

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Get script directory
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

echo "ears Test Suite"
echo "==============="
echo ""

# Check if bats is installed
if ! command -v bats &>/dev/null; then
    echo -e "${RED}Error: bats is not installed${NC}"
    echo ""
    echo "Install bats using one of these methods:"
    echo ""
    echo "  Ubuntu/Debian:"
    echo "    sudo apt install bats"
    echo ""
    echo "  From source:"
    echo "    git clone https://github.com/bats-core/bats-core.git"
    echo "    cd bats-core"
    echo "    sudo ./install.sh /usr/local"
    echo ""
    exit 1
fi

# Parse arguments
RUN_UNIT=1
RUN_INTEGRATION=1
VERBOSE=0
FILTER=""

while [[ $# -gt 0 ]]; do
    case $1 in
        --unit-only)
            RUN_INTEGRATION=0
            shift
            ;;
        --integration-only)
            RUN_UNIT=0
            shift
            ;;
        --verbose|-v)
            VERBOSE=1
            shift
            ;;
        --filter|-f)
            FILTER="$2"
            shift 2
            ;;
        --help|-h)
            echo "Usage: $0 [OPTIONS]"
            echo ""
            echo "Options:"
            echo "  --unit-only          Run only unit tests"
            echo "  --integration-only   Run only integration tests"
            echo "  --verbose, -v        Show verbose output"
            echo "  --filter PATTERN     Run only tests matching PATTERN"
            echo "  --help, -h           Show this help"
            exit 0
            ;;
        *)
            echo -e "${RED}Unknown option: $1${NC}"
            exit 1
            ;;
    esac
done

# Track results
TOTAL_TESTS=0
PASSED_TESTS=0
FAILED_TESTS=0

# Run tests in a category
run_test_category() {
    local category="$1"
    local test_dir="$SCRIPT_DIR/$category"

    if [[ ! -d "$test_dir" ]]; then
        echo -e "${YELLOW}No $category tests found${NC}"
        return
    fi

    echo -e "${GREEN}Running $category tests...${NC}"
    echo ""

    # Find all .bats files
    local test_files=()
    while IFS= read -r -d '' file; do
        if [[ -n "$FILTER" ]]; then
            if [[ "$file" =~ $FILTER ]]; then
                test_files+=("$file")
            fi
        else
            test_files+=("$file")
        fi
    done < <(find "$test_dir" -name "*.bats" -print0 | sort -z)

    if [[ ${#test_files[@]} -eq 0 ]]; then
        echo -e "${YELLOW}No matching test files found${NC}"
        echo ""
        return
    fi

    # Run each test file
    for test_file in "${test_files[@]}"; do
        local test_name=$(basename "$test_file")

        if [[ $VERBOSE -eq 1 ]]; then
            echo "Running: $test_name"
            if bats "$test_file"; then
                ((PASSED_TESTS++)) || true
            else
                ((FAILED_TESTS++)) || true
            fi
            ((TOTAL_TESTS++)) || true
        else
            # Capture output and only show on failure
            if output=$(bats "$test_file" 2>&1); then
                echo -e "  ${GREEN}✓${NC} $test_name"
                ((PASSED_TESTS++)) || true
            else
                echo -e "  ${RED}✗${NC} $test_name"
                echo "$output"
                ((FAILED_TESTS++)) || true
            fi
            ((TOTAL_TESTS++)) || true
        fi
    done

    echo ""
}

# Run unit tests
if [[ $RUN_UNIT -eq 1 ]]; then
    run_test_category "unit"
fi

# Run integration tests
if [[ $RUN_INTEGRATION -eq 1 ]]; then
    run_test_category "integration"
fi

# Print summary
echo "==============="
echo "Test Summary"
echo "==============="
echo "Total test files: $TOTAL_TESTS"
echo -e "Passed: ${GREEN}$PASSED_TESTS${NC}"
if [[ $FAILED_TESTS -gt 0 ]]; then
    echo -e "Failed: ${RED}$FAILED_TESTS${NC}"
else
    echo -e "Failed: $FAILED_TESTS"
fi
echo ""

if [[ $FAILED_TESTS -eq 0 ]]; then
    echo -e "${GREEN}All tests passed!${NC}"
    exit 0
else
    echo -e "${RED}Some tests failed${NC}"
    exit 1
fi
