#!/usr/bin/env bash
# Extract the branch name from `gh issue develop --list` output.
# Input:  tab-separated lines on stdin (branch<TAB>URL)
# Output: first branch name (first field of first line)
head -1 | cut -f1
