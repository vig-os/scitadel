"""Chat message display widgets for the research assistant."""

from __future__ import annotations

from textual.app import ComposeResult
from textual.containers import ScrollableContainer
from textual.widgets import Static


class UserMessage(Static):
    """A message from the user."""

    DEFAULT_CSS = """
    UserMessage {
        background: $primary-darken-3;
        color: $text;
        padding: 0 1;
        margin: 0 0 1 4;
        width: 1fr;
    }
    """

    def __init__(self, text: str) -> None:
        super().__init__(f"[bold]You:[/bold] {text}")


class AssistantMessage(Static):
    """A message from the assistant, supports incremental text append."""

    DEFAULT_CSS = """
    AssistantMessage {
        background: $surface-darken-1;
        color: $text;
        padding: 0 1;
        margin: 0 4 1 0;
        width: 1fr;
    }
    """

    def __init__(self) -> None:
        super().__init__("")
        self._text = ""

    def append_text(self, text: str) -> None:
        """Append text to this message (for streaming)."""
        self._text += text
        self.update(self._text)

    @property
    def text(self) -> str:
        return self._text


class ToolIndicator(Static):
    """Compact status indicator for a tool call."""

    DEFAULT_CSS = """
    ToolIndicator {
        color: $text-muted;
        padding: 0 1;
        margin: 0 2;
        height: 1;
    }
    """

    def __init__(self, tool_name: str, tool_id: str) -> None:
        self._tool_name = tool_name
        self._tool_id = tool_id
        label = _tool_display_name(tool_name)
        super().__init__(f"  [dim]{label}...[/dim]")

    def mark_complete(self, summary: str) -> None:
        """Update to show completion."""
        label = _tool_display_name(self._tool_name)
        self.update(f"  [dim]{label} — {summary}[/dim]")


class SystemMessage(Static):
    """System/error message in the chat."""

    DEFAULT_CSS = """
    SystemMessage {
        color: $warning;
        padding: 0 1;
        margin: 0 0 1 0;
    }
    """

    def __init__(self, text: str) -> None:
        super().__init__(f"[italic]{text}[/italic]")


class ChatMessageList(ScrollableContainer):
    """Scrollable container for chat messages, auto-scrolls to bottom."""

    DEFAULT_CSS = """
    ChatMessageList {
        height: 1fr;
        min-height: 10;
        border: solid $primary;
        padding: 1;
    }
    """

    def compose(self) -> ComposeResult:
        yield from ()

    def add_user_message(self, text: str) -> UserMessage:
        msg = UserMessage(text)
        self.mount(msg)
        self._scroll_to_bottom()
        return msg

    def add_assistant_message(self) -> AssistantMessage:
        msg = AssistantMessage()
        self.mount(msg)
        self._scroll_to_bottom()
        return msg

    def add_tool_indicator(self, tool_name: str, tool_id: str) -> ToolIndicator:
        indicator = ToolIndicator(tool_name, tool_id)
        self.mount(indicator)
        self._scroll_to_bottom()
        return indicator

    def add_system_message(self, text: str) -> SystemMessage:
        msg = SystemMessage(text)
        self.mount(msg)
        self._scroll_to_bottom()
        return msg

    def _scroll_to_bottom(self) -> None:
        self.call_later(self.scroll_end, animate=False)


def _tool_display_name(tool_name: str) -> str:
    """Human-friendly display name for a tool."""
    names = {
        "search": "Searching databases",
        "list_searches": "Listing searches",
        "get_papers": "Loading papers",
        "get_paper": "Loading paper details",
        "export_search": "Exporting results",
        "create_question": "Creating research question",
        "list_questions": "Listing questions",
        "add_search_terms": "Adding search terms",
        "assess_paper": "Scoring paper",
        "get_assessments": "Loading assessments",
        "snowball_search": "Running citation chaining",
    }
    return names.get(tool_name, tool_name)
