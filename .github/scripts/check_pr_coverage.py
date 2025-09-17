#!/usr/bin/env python3

import os
import subprocess
import sys
import xml.etree.ElementTree as ET
from collections import defaultdict


def get_pr_diff(base_branch):
    """
    Get the diff of the current branch against the base branch and return a
    dictionary mapping file paths to a set of added or modified line numbers.
    """
    diff_files = defaultdict(set)
    # We fetch the base branch to ensure it's available for diffing
    subprocess.run(
        ["git", "fetch", "origin", base_branch],
        check=True,
        capture_output=True,
    )
    result = subprocess.run(
        ["git", "diff", f"origin/{base_branch}", "--unified=0"],
        capture_output=True,
        text=True,
        check=True,
    )
    file_path = None
    for line in result.stdout.splitlines():
        if line.startswith("+++ b/"):
            file_path = line[6:]
        elif line.startswith("@@"):
            parts = line.split(" ")
            if len(parts) > 2 and parts[2].startswith("+"):
                line_info = parts[2][1:].split(",")
                start_line = int(line_info[0])
                num_lines = int(line_info[1]) if len(line_info) > 1 else 1
                for i in range(num_lines):
                    diff_files[file_path].add(start_line + i)
    return diff_files


def get_uncovered_lines(cobertura_path):
    """
    Parse the Cobertura XML report and return a dictionary mapping file paths
    to a set of uncovered line numbers.
    """
    uncovered_lines = defaultdict(set)
    if not os.path.exists(cobertura_path):
        msg = (
            f"Error: Coverage report not found at {cobertura_path}"
        )
        print(msg, file=sys.stderr)
        sys.exit(1)

    tree = ET.parse(cobertura_path)
    root = tree.getroot()
    packages = root.find("packages")
    if packages is None:
        return uncovered_lines

    for package in packages.findall("package"):
        classes = package.find("classes")
        if classes is None:
            continue
        for klass in classes.findall("class"):
            file_path = klass.attrib["filename"]
            lines = klass.find("lines")
            if lines is None:
                continue
            for line in lines.findall("line"):
                if line.attrib["hits"] == "0":
                    uncovered_lines[file_path].add(int(line.attrib["number"]))
    return uncovered_lines


def get_covered_lines(cobertura_path):
    """
    Parse the Cobertura XML report and return a dictionary mapping file paths
    to a set of covered line numbers (hits > 0).
    """
    covered_lines = defaultdict(set)
    if not os.path.exists(cobertura_path):
        msg = (
            f"Error: Coverage report not found at {cobertura_path}"
        )
        print(msg, file=sys.stderr)
        sys.exit(1)

    tree = ET.parse(cobertura_path)
    root = tree.getroot()
    packages = root.find("packages")
    if packages is None:
        return covered_lines

    for package in packages.findall("package"):
        classes = package.find("classes")
        if classes is None:
            continue
        for klass in classes.findall("class"):
            file_path = klass.attrib["filename"]
            lines = klass.find("lines")
            if lines is None:
                continue
            for line in lines.findall("line"):
                if int(line.attrib.get("hits", "0")) > 0:
                    covered_lines[file_path].add(int(line.attrib["number"]))
    return covered_lines

def to_ranges(numbers):
    """
    Convert a list of numbers into a string of comma-separated ranges.
    e.g., [1, 2, 3, 5, 6, 8] -> "1-3, 5-6, 8"
    """
    if not numbers:
        return ""
    numbers = sorted(list(set(numbers)))
    ranges = []
    start = end = numbers[0]
    for n in numbers[1:]:
        if n == end + 1:
            end = n
        else:
            if start == end:
                ranges.append(str(start))
            else:
                ranges.append(f"{start}-{end}")
            start = end = n
    if start == end:
        ranges.append(str(start))
    else:
        ranges.append(f"{start}-{end}")
    return ", ".join(ranges)

def main():
    if len(sys.argv) < 2:
        print("Usage: python check_pr_coverage.py <base-branch>", file=sys.stderr)
        sys.exit(1)

    base_branch = sys.argv[1]
    cobertura_xml_path = "./coverage-reports/cobertura.xml"

    try:
        pr_diff = get_pr_diff(base_branch)
        covered_lines = get_covered_lines(cobertura_xml_path)
        uncovered_lines = get_uncovered_lines(cobertura_xml_path)
    except (subprocess.CalledProcessError, FileNotFoundError) as e:
        print(f"Error: {e}", file=sys.stderr)
        sys.exit(1)

    # Compute changed-lines-only coverage
    total_coverable_changed_lines = 0
    total_covered_changed_lines = 0

    uncovered_in_pr = defaultdict(list)
    for file_path, changed_lines in pr_diff.items():
        file_covered = covered_lines.get(file_path, set())
        file_uncovered = uncovered_lines.get(file_path, set())
        valid_coverable = file_covered.union(file_uncovered)

        coverable_in_changes = changed_lines.intersection(valid_coverable)
        total_coverable_changed_lines += len(coverable_in_changes)
        total_covered_changed_lines += len(changed_lines.intersection(file_covered))

        if file_path in uncovered_lines:
            intersection = sorted(
                list(
                    changed_lines.intersection(
                        uncovered_lines[file_path]
                    )
                )
            )
            if intersection:
                uncovered_in_pr[file_path] = intersection

    # Print changed-lines-only coverage summary at the top
    if total_coverable_changed_lines > 0:
        pct = (
            total_covered_changed_lines / total_coverable_changed_lines
        ) * 100.0
        print(
            f"### 📈 Changed lines coverage: {pct:.2f}% "
            f"({total_covered_changed_lines}/{total_coverable_changed_lines})\n"
        )
    else:
        print("### 📈 Changed lines coverage: N/A (0/0)\n")

    if uncovered_in_pr:
        print("### 🚨 Uncovered lines in this PR\n")
        for file_path, lines in uncovered_in_pr.items():
            lines_str = to_ranges(lines)
            print(f"- **{file_path}:** `{lines_str}`")
    else:
        print("### ✅ All new and modified lines are covered by tests!")



if __name__ == "__main__":
    main()
