#!/usr/bin/env python3
"""
Cross-language test runner for ears

Runs tests defined in JSON format against both Bash and Rust implementations.
"""

import argparse
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Dict, List, Any, Optional


class TestResult:
    def __init__(self, name: str, passed: bool, message: str = ""):
        self.name = name
        self.passed = passed
        self.message = message


class TestRunner:
    def __init__(self, implementation: str, verbose: bool = False):
        self.implementation = implementation
        self.verbose = verbose
        self.repo_root = Path(__file__).parent.parent.parent

        if implementation == "bash":
            self.binary = self.repo_root / "bin" / "ears"
        elif implementation == "rust":
            # Check if release build exists, otherwise use debug
            release_bin = self.repo_root / "target" / "release" / "ears"
            debug_bin = self.repo_root / "target" / "debug" / "ears"
            if release_bin.exists():
                self.binary = release_bin
            elif debug_bin.exists():
                self.binary = debug_bin
            else:
                raise RuntimeError(
                    "Rust binary not found. Run 'cargo build' first."
                )
        else:
            raise ValueError(f"Unknown implementation: {implementation}")

        if not self.binary.exists():
            raise RuntimeError(f"Binary not found: {self.binary}")

    def run_test(self, test_path: Path) -> TestResult:
        """Run a single test case"""
        with open(test_path) as f:
            test = json.load(f)

        name = test.get("name", test_path.name)

        if self.verbose:
            print(f"\n  Running: {name}")

        # Create temporary test environment
        with tempfile.TemporaryDirectory() as tmpdir:
            try:
                # Setup environment
                env = os.environ.copy()

                # Add mocks directory to PATH for bash implementation
                if self.implementation == "bash":
                    mocks_dir = self.repo_root / "tests" / "mocks"
                    env["PATH"] = f"{mocks_dir}:{env.get('PATH', '')}"

                setup = test.get("setup", {})

                # Set environment variables
                test_env = setup.get("env", {})
                for key, value in test_env.items():
                    # Replace $TMPDIR in values
                    value = value.replace("$TMPDIR", tmpdir)
                    env[key] = value

                # Create files
                files = setup.get("files", {})
                for file_path, content in files.items():
                    file_path = file_path.replace("$TMPDIR", tmpdir)
                    file_path = Path(file_path)
                    file_path.parent.mkdir(parents=True, exist_ok=True)
                    file_path.write_text(content)

                # Run command
                command = test.get("command", {})
                args = command.get("args", [])
                stdin = command.get("stdin")

                cmd = [str(self.binary)] + args

                result = subprocess.run(
                    cmd,
                    env=env,
                    capture_output=True,
                    text=True,
                    input=stdin,
                    timeout=10
                )

                # Check assertions
                assertions = test.get("assertions", {})

                # Exit code
                expected_exit = assertions.get("exit_code", 0)
                if result.returncode != expected_exit:
                    return TestResult(
                        name,
                        False,
                        f"Expected exit code {expected_exit}, got {result.returncode}\n"
                        f"stdout: {result.stdout}\nstderr: {result.stderr}"
                    )

                # Stdout contains
                for pattern in assertions.get("stdout_contains", []):
                    if pattern not in result.stdout:
                        return TestResult(
                            name,
                            False,
                            f"Expected stdout to contain '{pattern}'\n"
                            f"Got: {result.stdout}"
                        )

                # Stdout not contains
                for pattern in assertions.get("stdout_not_contains", []):
                    if pattern in result.stdout:
                        return TestResult(
                            name,
                            False,
                            f"Expected stdout to NOT contain '{pattern}'\n"
                            f"Got: {result.stdout}"
                        )

                # Stderr contains
                for pattern in assertions.get("stderr_contains", []):
                    if pattern not in result.stderr:
                        return TestResult(
                            name,
                            False,
                            f"Expected stderr to contain '{pattern}'\n"
                            f"Got: {result.stderr}"
                        )

                # Files exist
                for file_path in assertions.get("files_exist", []):
                    file_path = file_path.replace("$TMPDIR", tmpdir)
                    if not Path(file_path).exists():
                        return TestResult(
                            name, False, f"Expected file to exist: {file_path}"
                        )

                # Files not exist
                for file_path in assertions.get("files_not_exist", []):
                    file_path = file_path.replace("$TMPDIR", tmpdir)
                    if Path(file_path).exists():
                        return TestResult(
                            name, False, f"Expected file to NOT exist: {file_path}"
                        )

                # File contains
                for file_path, expected_content in assertions.get(
                    "file_contains", {}
                ).items():
                    file_path = file_path.replace("$TMPDIR", tmpdir)
                    if not Path(file_path).exists():
                        return TestResult(
                            name, False, f"File does not exist: {file_path}"
                        )
                    actual_content = Path(file_path).read_text().strip()
                    if expected_content not in actual_content:
                        return TestResult(
                            name,
                            False,
                            f"Expected file {file_path} to contain '{expected_content}'\n"
                            f"Got: {actual_content}"
                        )

                return TestResult(name, True)

            except subprocess.TimeoutExpired:
                return TestResult(name, False, "Test timed out")
            except Exception as e:
                return TestResult(name, False, f"Error: {e}")

    def run_all_tests(self, test_dir: Path) -> List[TestResult]:
        """Run all tests in a directory"""
        results = []

        # Find all JSON test files
        test_files = sorted(test_dir.rglob("*.json"))

        if not test_files:
            print(f"No test files found in {test_dir}")
            return results

        print(f"\nRunning {len(test_files)} tests against {self.implementation} implementation:")

        for test_file in test_files:
            result = self.run_test(test_file)
            results.append(result)

            # Print result
            status = "✓" if result.passed else "✗"
            print(f"  {status} {result.name}")

            if not result.passed and self.verbose:
                print(f"    {result.message}")

        return results


def main():
    parser = argparse.ArgumentParser(
        description="Run cross-language tests for ears"
    )
    parser.add_argument(
        "--impl",
        choices=["bash", "rust", "both"],
        default="both",
        help="Which implementation to test"
    )
    parser.add_argument(
        "--verbose",
        "-v",
        action="store_true",
        help="Verbose output"
    )
    parser.add_argument(
        "--test-dir",
        type=Path,
        default=Path(__file__).parent / "tests",
        help="Directory containing test files"
    )

    args = parser.parse_args()

    implementations = ["bash", "rust"] if args.impl == "both" else [args.impl]

    all_results = {}

    for impl in implementations:
        try:
            runner = TestRunner(impl, args.verbose)
            results = runner.run_all_tests(args.test_dir)
            all_results[impl] = results
        except RuntimeError as e:
            print(f"\nError testing {impl}: {e}")
            all_results[impl] = []

    # Print summary
    print("\n" + "=" * 60)
    print("Summary:")
    print("=" * 60)

    for impl, results in all_results.items():
        if not results:
            print(f"\n{impl.upper()}: No tests run")
            continue

        passed = sum(1 for r in results if r.passed)
        total = len(results)
        percentage = (passed / total * 100) if total > 0 else 0

        print(f"\n{impl.upper()}: {passed}/{total} passed ({percentage:.1f}%)")

        # Show failures
        failures = [r for r in results if not r.passed]
        if failures:
            print(f"\nFailed tests:")
            for result in failures:
                print(f"  ✗ {result.name}")
                if args.verbose:
                    print(f"    {result.message}")

    # Exit with error if any tests failed
    all_passed = all(
        all(r.passed for r in results)
        for results in all_results.values()
    )
    sys.exit(0 if all_passed else 1)


if __name__ == "__main__":
    main()
