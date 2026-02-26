#!/usr/bin/env python3
"""Display open GitHub issues and pull requests in rich tables."""

from __future__ import annotations

import json
import re
import subprocess
import sys
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from rich.table import Table


def _fetch_issues() -> list[dict]:
    result = subprocess.run(
        [
            "gh",
            "issue",
            "list",
            "--state",
            "open",
            "--limit",
            "200",
            "--json",
            "number,title,state,assignees,labels,milestone",
        ],
        capture_output=True,
        text=True,
        check=True,
    )
    return json.loads(result.stdout)


_LINKED_BRANCHES_QUERY = """
{
  repository(owner: "%s", name: "%s") {
    issues(states: OPEN, first: 100) {
      nodes {
        number
        linkedBranches(first: 5) {
          nodes { ref { name } }
        }
      }
    }
  }
}
"""


def _fetch_linked_branches() -> dict[int, str]:
    """Return {issue_number: branch_name} for issues with a linked branch."""
    owner_result = subprocess.run(
        ["gh", "repo", "view", "--json", "owner,name"],
        capture_output=True,
        text=True,
        check=True,
    )
    repo_info = json.loads(owner_result.stdout)
    query = _LINKED_BRANCHES_QUERY % (repo_info["owner"]["login"], repo_info["name"])

    result = subprocess.run(
        ["gh", "api", "graphql", "-f", f"query={query}"],
        capture_output=True,
        text=True,
        check=True,
    )
    data = json.loads(result.stdout)
    mapping: dict[int, str] = {}
    for node in data["data"]["repository"]["issues"]["nodes"]:
        branches = [
            b["ref"]["name"]
            for b in node["linkedBranches"]["nodes"]
            if b.get("ref") and b["ref"].get("name")
        ]
        if branches:
            mapping[node["number"]] = branches[0]
    return mapping


def _fetch_sub_issue_tree(
    issues: list[dict],
    owner_repo: str,
) -> tuple[dict[int, int], dict[int, list[int]]]:
    """Return (child_to_parent, parent_to_children) for open issues.

    Queries each issue's parent endpoint in parallel. Output format is one
    ``child:parent`` pair per line; 404s (no parent) are silently skipped.
    """
    child_to_parent: dict[int, int] = {}
    parent_to_children: dict[int, list[int]] = {}

    nums = " ".join(str(i["number"]) for i in issues)
    script = (
        f'echo {nums} | tr " " "\\n" | '
        "xargs -P8 -I{} sh -c '"
        "p=$(gh api repos/" + owner_repo + "/issues/{}/parent "
        "--jq .number 2>/dev/null) && "
        '[ -n "$p" ] && echo "{}:$p" || true\''
    )
    result = subprocess.run(
        ["sh", "-c", script],
        capture_output=True,
        text=True,
    )

    for line in result.stdout.strip().split("\n"):
        line = line.strip()
        if ":" not in line:
            continue
        parts = line.split(":", 1)
        if len(parts) == 2 and parts[0].isdigit() and parts[1].isdigit():
            child, parent = int(parts[0]), int(parts[1])
            child_to_parent[child] = parent
            parent_to_children.setdefault(parent, []).append(child)

    for parent in parent_to_children:
        parent_to_children[parent].sort()

    return child_to_parent, parent_to_children


LABEL_STYLES: dict[str, str] = {
    "priority:critical": "bold red",
    "priority:high": "red",
    "priority:medium": "yellow",
    "priority:low": "dim",
    "priority:backlog": "dim italic",
    "effort:small": "green",
    "effort:medium": "yellow",
    "effort:large": "red",
    "semver:major": "bold red",
    "semver:minor": "yellow",
    "semver:patch": "green",
}

TYPE_STYLES: dict[str, str] = {
    "feature": "cyan",
    "bug": "bold red",
    "discussion": "bright_magenta",
    "chore": "dim",
}

AREA_STYLE = "blue"


def _styled(value: str, style: str) -> str:
    return f"[{style}]{value}[/]"


def _gh_link(owner_repo: str, num: int, kind: str) -> str:
    """Return Rich hyperlink markup for issue or PR number."""
    return f"[link=https://github.com/{owner_repo}/{kind}/{num}]{num}[/link]"


def _extract_label(labels: list[dict], prefix: str) -> str:
    for lbl in labels:
        name = lbl["name"]
        if name.startswith(prefix):
            val = name[len(prefix) :]
            style = LABEL_STYLES.get(name, "dim")
            return _styled(val, style)
    return ""


def _extract_type(labels: list[dict]) -> str:
    for lbl in labels:
        name = lbl["name"]
        if name in TYPE_STYLES:
            return _styled(name, TYPE_STYLES[name])
    return ""


def _extract_scope(labels: list[dict]) -> str:
    scopes = []
    for lbl in labels:
        name = lbl["name"]
        if name.startswith("area:"):
            scopes.append(_styled(name[5:], AREA_STYLE))
    return ", ".join(scopes)


_TITLE_PREFIX_RE = re.compile(r"^\[(FEATURE|TASK|BUG|DISCUSSION|CHORE)\]\s*")


def _clean_title(title: str) -> str:
    return _TITLE_PREFIX_RE.sub("", title)


def _format_assignees(assignees: list[dict]) -> str:
    if not assignees:
        return "[dim]—[/]"
    return ", ".join(f"[bright_white]{a['login']}[/]" for a in assignees)


_CLOSING_RE = re.compile(r"(?:closes|fixes|resolves)\s+#(\d+)", re.IGNORECASE)
_REFS_RE = re.compile(r"Refs:\s*((?:#\d+(?:\s*,\s*)?)+)", re.IGNORECASE)


def _build_cross_refs(
    branches: dict[int, str],
    prs: list[dict],
) -> tuple[dict[int, int], dict[int, list[int]]]:
    """Build issue-PR cross-references from branch names and PR body keywords.

    Returns (issue_to_pr, pr_to_issues) mappings.
    """
    branch_to_issue = {branch: num for num, branch in branches.items()}
    issue_to_pr: dict[int, int] = {}
    pr_to_issues: dict[int, list[int]] = {}
    for pr in prs:
        pr_num = pr["number"]
        linked: set[int] = set()

        head = pr["headRefName"]
        issue_num = branch_to_issue.get(head)
        if issue_num is not None:
            linked.add(issue_num)

        body = pr.get("body") or ""
        for match in _CLOSING_RE.finditer(body):
            linked.add(int(match.group(1)))
        refs_match = _REFS_RE.search(body)
        if refs_match:
            for m in re.finditer(r"#(\d+)", refs_match.group(1)):
                linked.add(int(m.group(1)))

        for inum in linked:
            issue_to_pr[inum] = pr_num
        if linked:
            pr_to_issues[pr_num] = sorted(linked)
    return issue_to_pr, pr_to_issues


def _build_table(
    title: str,
    issues: list[dict],
    branches: dict[int, str],
    issue_to_pr: dict[int, int],
    child_to_parent: dict[int, int],
    parent_to_children: dict[int, list[int]],
    owner_repo: str,
) -> Table:
    from rich.table import Table

    table = Table(
        title=title,
        title_style="bold",
        show_lines=False,
        pad_edge=True,
        expand=True,
        border_style="dim",
        title_justify="left",
    )
    table.add_column("#", style="bold cyan", no_wrap=True, justify="right", width=4)
    table.add_column("Type", no_wrap=True, width=7)
    table.add_column(
        "Title",
        no_wrap=True,
        overflow="ellipsis",
        min_width=20,
        ratio=1,
    )
    table.add_column("Assignee", no_wrap=True, max_width=14)
    table.add_column(
        "Branch",
        style="dim",
        no_wrap=True,
        overflow="ellipsis",
        max_width=24,
    )
    table.add_column("PR", no_wrap=True, justify="right", width=4)
    table.add_column("Prio", no_wrap=True, justify="center", width=7)
    table.add_column("Scope", no_wrap=True, width=9)
    table.add_column("Effort", no_wrap=True, justify="center", width=6)
    table.add_column("SemVer", no_wrap=True, justify="center", width=5)

    issue_by_num = {i["number"]: i for i in issues}

    rendered: set[int] = set()
    sorted_issues = sorted(issues, key=lambda i: i["number"])

    def _add_row(issue: dict, *, indent: int = 0) -> None:
        num = issue["number"]
        if num in rendered:
            return
        rendered.add(num)
        labels = issue.get("labels", [])
        branch = branches.get(num, "")
        pr_num = issue_to_pr.get(num)
        pr_cell = _styled(f"#{pr_num}", "green") if pr_num else ""

        title_text = _clean_title(issue["title"])
        if indent > 0:
            title_text = _styled(f"└ {title_text}", "dim")
        elif num in parent_to_children:
            title_text = _styled(f"▸ {title_text}", "bright_cyan")

        table.add_row(
            _gh_link(owner_repo, num, "issues"),
            _extract_type(labels),
            title_text,
            _format_assignees(issue["assignees"]),
            branch,
            pr_cell,
            _extract_label(labels, "priority:"),
            _extract_scope(labels),
            _extract_label(labels, "effort:"),
            _extract_label(labels, "semver:"),
        )

    for issue in sorted_issues:
        num = issue["number"]
        if num in rendered:
            continue
        if num in child_to_parent and child_to_parent[num] in issue_by_num:
            continue
        _add_row(issue, indent=0)
        for child_num in parent_to_children.get(num, []):
            if child_num in issue_by_num:
                _add_row(issue_by_num[child_num], indent=1)

    for issue in sorted_issues:
        if issue["number"] not in rendered:
            _add_row(issue, indent=0)

    return table


def _fetch_prs() -> list[dict]:
    result = subprocess.run(
        [
            "gh",
            "pr",
            "list",
            "--state",
            "open",
            "--limit",
            "100",
            "--json",
            "number,title,author,assignees,isDraft,reviewDecision,"
            "baseRefName,headRefName,additions,deletions,changedFiles,"
            "labels,milestone,createdAt,body,"
            "reviewRequests,latestReviews,statusCheckRollup",
        ],
        capture_output=True,
        text=True,
        check=True,
    )
    return json.loads(result.stdout)


REVIEW_STYLES: dict[str, tuple[str, str]] = {
    "APPROVED": ("green", "approved"),
    "CHANGES_REQUESTED": ("red", "changes"),
    "REVIEW_REQUIRED": ("yellow", "pending"),
}


def _infer_review(pr: dict) -> tuple[str, str]:
    """Return (state, label) using reviewDecision, falling back to latestReviews."""
    decision = pr.get("reviewDecision") or ""
    if decision:
        entry = REVIEW_STYLES.get(decision)
        return (decision, entry[1]) if entry else (decision, decision.lower())
    latest = pr.get("latestReviews") or []
    if latest:
        best = latest[-1].get("state", "")
        entry = REVIEW_STYLES.get(best)
        return (best, entry[1]) if entry else (best, best.lower())
    if pr.get("reviewRequests"):
        return ("REVIEW_REQUIRED", "pending")
    return ("", "—")


def _dedupe_status_checks(rollup: list[dict]) -> list[dict]:
    """Deduplicate statusCheckRollup by check name, keeping latest by completedAt.

    GitHub includes re-runs of the same check; we keep only the latest result
    per check name so the CI column matches what GitHub shows on the PR page.
    Ref: #176
    """
    by_name: dict[str, dict] = {}
    for check in rollup:
        name = check.get("name") or "?"
        completed = check.get("completedAt") or ""
        existing = by_name.get(name)
        if existing is None:
            by_name[name] = check
        else:
            existing_completed = existing.get("completedAt") or ""
            if completed >= existing_completed:
                by_name[name] = check
    return list(by_name.values())


def _format_ci_status(pr: dict, owner_repo: str) -> str:
    """Return Rich markup for CI status cell: pass/fail/pending summary with link.

    Uses statusCheckRollup from gh pr list. Links to PR checks tab.
    Ref: #143
    """
    rollup = _dedupe_status_checks(pr.get("statusCheckRollup") or [])
    if not rollup:
        return _styled("—", "dim")

    total = len(rollup)
    passed = sum(1 for c in rollup if c.get("conclusion") == "SUCCESS")
    failed = sum(1 for c in rollup if c.get("conclusion") in ("FAILURE", "ERROR"))
    pending = total - passed - failed

    url = f"https://github.com/{owner_repo}/pull/{pr['number']}/checks"
    link_prefix = f"[link={url}]"
    link_suffix = "[/link]"

    if failed > 0:
        failed_names = [
            c.get("name", "?")
            for c in rollup
            if c.get("conclusion") in ("FAILURE", "ERROR")
        ]
        text = f"✗ {passed}/{total} {', '.join(failed_names)}"
        return link_prefix + _styled(text, "red") + link_suffix
    if pending > 0:
        text = f"⏳ {passed}/{total}"
        return link_prefix + _styled(text, "yellow") + link_suffix
    text = f"✓ {passed}/{total}"
    return link_prefix + _styled(text, "green") + link_suffix


def _extract_reviewers(pr: dict) -> str:
    """Build a compact reviewer string from latestReviews and reviewRequests."""
    seen: dict[str, str] = {}
    for r in pr.get("latestReviews") or []:
        login = (r.get("author") or {}).get("login", "")
        if login:
            state = r.get("state", "")
            seen[login] = state
    for r in pr.get("reviewRequests") or []:
        login = r.get("login") or ""
        if not login:
            login = r.get("name") or ""
        if login and login not in seen:
            seen[login] = "REQUESTED"
    if not seen:
        return _styled("—", "dim")
    parts = []
    for login, state in seen.items():
        if state == "APPROVED":
            parts.append(_styled(login, "green"))
        elif state == "CHANGES_REQUESTED":
            parts.append(_styled(login, "red"))
        elif state == "REQUESTED":
            parts.append(_styled(f"?{login}", "dim italic"))
        else:
            parts.append(_styled(login, "yellow"))
    return " ".join(parts)


def _build_pr_table(
    title: str,
    prs: list[dict],
    pr_to_issues: dict[int, list[int]],
    owner_repo: str,
) -> Table:
    from rich.table import Table

    table = Table(
        title=title,
        title_style="bold",
        show_lines=False,
        pad_edge=True,
        expand=True,
        border_style="dim",
        title_justify="left",
    )
    table.add_column("#", style="bold cyan", no_wrap=True, justify="right", width=4)
    table.add_column("Title", no_wrap=True, overflow="ellipsis", ratio=1)
    table.add_column("Author", no_wrap=True, width=12)
    table.add_column("Assignee", no_wrap=True, width=12)
    table.add_column("Issues", no_wrap=True, width=10)
    table.add_column("Branch", no_wrap=True, overflow="ellipsis", max_width=30)
    table.add_column("CI", no_wrap=True, justify="center", width=14)
    table.add_column("Review", no_wrap=True, justify="center", width=8)
    table.add_column("Reviewer", no_wrap=True, width=12)
    table.add_column("Delta", no_wrap=True, justify="right", width=14)

    for pr in sorted(prs, key=lambda p: p["number"]):
        review_state, review_label = _infer_review(pr)
        style, label = REVIEW_STYLES.get(
            review_state,
            ("dim", review_label or "—"),
        )
        review = _styled(label, style)
        reviewer = _extract_reviewers(pr)

        draft_marker = _styled(" draft", "dim italic") if pr.get("isDraft") else ""

        adds = pr.get("additions", 0)
        dels = pr.get("deletions", 0)
        files = pr.get("changedFiles", 0)
        delta = f"[green]+{adds}[/] [red]-{dels}[/] [dim]{files}f[/]"

        branch = f"[dim]{pr['headRefName']}[/] → [dim]{pr['baseRefName']}[/]"

        linked = pr_to_issues.get(pr["number"], [])
        issues_cell = (
            " ".join(_gh_link(owner_repo, n, "issues") for n in sorted(linked))
            if linked
            else ""
        )

        ci_cell = _format_ci_status(pr, owner_repo)

        table.add_row(
            _gh_link(owner_repo, pr["number"], "pull"),
            _clean_title(pr["title"]) + draft_marker,
            f"[bright_white]{pr['author']['login']}[/]",
            _format_assignees(pr.get("assignees", [])),
            issues_cell,
            branch,
            ci_cell,
            review,
            reviewer,
            delta,
        )
    return table


def main() -> int:
    from rich.console import Console

    issues = _fetch_issues()
    prs = _fetch_prs()
    branches = _fetch_linked_branches() if issues else {}
    issue_to_pr, pr_to_issues = _build_cross_refs(branches, prs)

    owner_result = subprocess.run(
        ["gh", "repo", "view", "--json", "nameWithOwner", "--jq", ".nameWithOwner"],
        capture_output=True,
        text=True,
        check=True,
    )
    owner_repo = owner_result.stdout.strip()
    child_to_parent, parent_to_children = (
        _fetch_sub_issue_tree(issues, owner_repo) if issues else ({}, {})
    )

    console = Console()

    # --- Issues ---
    if issues:
        milestones: dict[str, list[dict]] = {}
        no_milestone: list[dict] = []

        for issue in issues:
            ms = issue.get("milestone")
            if ms and ms.get("title"):
                milestones.setdefault(ms["title"], []).append(issue)
            else:
                no_milestone.append(issue)

        console.print()
        console.rule(f"[bold]Open Issues ({len(issues)})[/]")

        for ms_title in sorted(milestones):
            group = milestones[ms_title]
            table = _build_table(
                f"[cyan]▸ Milestone {ms_title}[/]  [dim]({len(group)} issues)[/]",
                group,
                branches,
                issue_to_pr,
                child_to_parent,
                parent_to_children,
                owner_repo,
            )
            console.print()
            console.print(table)

        if no_milestone:
            table = _build_table(
                f"[yellow]▸ No Milestone[/]  [dim]({len(no_milestone)} issues)[/]",
                no_milestone,
                branches,
                issue_to_pr,
                child_to_parent,
                parent_to_children,
                owner_repo,
            )
            console.print()
            console.print(table)
    else:
        console.print()
        console.print("[dim]No open issues.[/]")

    # --- Pull Requests ---
    console.print()
    if prs:
        console.rule(f"[bold]Open Pull Requests ({len(prs)})[/]")
        table = _build_pr_table(
            f"[green]▸ Pull Requests[/]  [dim]({len(prs)} open)[/]",
            prs,
            pr_to_issues,
            owner_repo,
        )
        console.print()
        console.print(table)
    else:
        console.rule("[bold]Pull Requests[/]")
        console.print("[dim]No open pull requests.[/]")

    console.print()
    return 0


if __name__ == "__main__":
    sys.exit(main())
