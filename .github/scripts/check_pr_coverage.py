#!/usr/bin/env python3

import os
import subprocess
import sys
import xml.etree.ElementTree as ET
from collections import defaultdict


def get_pr_diff(base_branch):
    """
    Get the diff of the current branch against the base branch and return a dictionary
    mapping file paths to a set of added or modified line numbers.
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
        print(f"Error: Coverage report not found at {cobertura_path}", file=sys.stderr)
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

def main():
    if len(sys.argv) < 2:
        print("Usage: python check_pr_coverage.py <base-branch>", file=sys.stderr)
        sys.exit(1)

    base_branch = sys.argv[1]
    cobertura_xml_path = "./coverage-reports/cobertura.xml"

    try:
        pr_diff = get_pr_diff(base_branch)
        uncovered_lines = get_uncovered_lines(cobertura_xml_path)
    except (subprocess.CalledProcessError, FileNotFoundError) as e:
        print(f"Error: {e}", file=sys.stderr)
        sys.exit(1)

    uncovered_in_pr = defaultdict(list)
    for file_path, changed_lines in pr_diff.items():
        if file_path in uncovered_lines:
            intersection = sorted(list(changed_lines.intersection(uncovered_lines[file_path])))
            if intersection:
                uncovered_in_pr[file_path] = intersection

    if uncovered_in_pr:
        print("### 🚨 Uncovered lines in this PR\n")
        for file_path, lines in uncovered_in_pr.items():
            lines_str = ", ".join(map(str, lines))
            print(f"- **{file_path}:** `{lines_str}`")
    else:
        print("### ✅ All new and modified lines are covered by tests!")


if __name__ == "__main__":
    main()
