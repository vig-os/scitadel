"""Research Assistant — chat-driven research interface.

Composite widget: chat messages + input + terms bar + results table.
"""

from __future__ import annotations

import json
import logging

from textual.app import ComposeResult
from textual.containers import Horizontal, Vertical
from textual.widgets import Button, Input

import anthropic

from scitadel.config import load_config
from scitadel.services.chat_engine import (
    ChatEngine,
    ChatEvent,
    MissingAPIKeyError,
    TextDelta,
    ToolCallResult,
    ToolCallStart,
    TurnComplete,
)
from scitadel.tui.widgets.chat_message import ChatMessageList, AssistantMessage
from scitadel.tui.widgets.results_table import ResultsTable
from scitadel.tui.widgets.terms_bar import TermsBar

logger = logging.getLogger(__name__)


class ResearchAssistant(Vertical):
    """Chat-driven research assistant tab.

    Layout:
        Chat messages (scrollable)
        [Input box] [Send]
        Terms bar
        Results table (sortable)
    """

    DEFAULT_CSS = """
    ResearchAssistant {
        height: 1fr;
    }
    #chat-input-row {
        height: 3;
    }
    #chat-input {
        width: 1fr;
    }
    #chat-send {
        width: 10;
    }
    """

    def __init__(self) -> None:
        super().__init__()
        self._engine: ChatEngine | None = None
        self._current_assistant_msg: AssistantMessage | None = None
        self._tool_indicators: dict[str, object] = {}
        self._busy = False
        self._awaiting_api_key = False
        self._retry_message: str | None = None

    def compose(self) -> ComposeResult:
        yield ChatMessageList(id="chat-messages")
        yield TermsBar()
        with Horizontal(id="chat-input-row"):
            yield Input(
                placeholder="Describe your research topic...",
                id="chat-input",
            )
            yield Button("Send", id="chat-send", variant="primary")
        yield ResultsTable()

    def on_mount(self) -> None:
        config = load_config()
        store = self.app.store
        self._engine = ChatEngine(
            store=store,
            model=config.chat.model,
            max_tokens=config.chat.max_tokens,
        )

    def on_button_pressed(self, event: Button.Pressed) -> None:
        if event.button.id == "chat-send":
            self._submit_message()

    def on_input_submitted(self, event: Input.Submitted) -> None:
        if event.input.id == "chat-input":
            self._submit_message()

    def _submit_message(self) -> None:
        inp = self.query_one("#chat-input", Input)
        text = inp.value.strip()
        if not text or self._busy:
            return
        inp.value = ""

        # If we're waiting for an API key, handle that instead
        if self._awaiting_api_key:
            self._store_api_key(text)
            return

        chat = self.query_one("#chat-messages", ChatMessageList)
        chat.add_user_message(text)

        self._busy = True
        self.run_worker(self._process_message(text), thread=False)

    async def _process_message(self, text: str) -> None:
        """Run the chat engine and process events."""
        chat = self.query_one("#chat-messages", ChatMessageList)

        try:
            async for event in self._engine.send(text):
                self._handle_event(event, chat)
        except MissingAPIKeyError:
            self._engine._messages.pop()  # remove dangling user msg
            self._retry_message = text
            self._show_api_key_prompt(chat)
        except anthropic.AuthenticationError:
            # Invalid key — clear it and re-prompt
            from scitadel.secrets import delete_api_key

            delete_api_key()
            self._engine._client = None
            self._engine._messages.pop()  # remove dangling user msg
            self._retry_message = text
            chat.add_system_message("Invalid API key.")
            self._show_api_key_prompt(chat)
        except anthropic.BadRequestError as exc:
            self._engine._messages.pop()
            chat.add_system_message(_friendly_api_error(exc))
        except anthropic.RateLimitError:
            self._engine._messages.pop()
            chat.add_system_message("Rate limited. Please wait a moment and try again.")
        except Exception as exc:
            logger.exception("Chat engine error")
            chat.add_system_message(_friendly_api_error(exc))
        finally:
            self._busy = False
            self._current_assistant_msg = None

    def _show_api_key_prompt(self, chat: ChatMessageList) -> None:
        """Prompt the user to paste their API key inline."""
        inp = self.query_one("#chat-input", Input)
        self._awaiting_api_key = True
        chat.add_system_message(
            "No Anthropic API key found. "
            "Paste your key below (it will be stored securely in your "
            "system keychain):"
        )
        inp.placeholder = "sk-ant-..."
        inp.password = True
        inp.focus()

    def _store_api_key(self, key: str) -> None:
        """Validate and store the API key, then retry the pending message."""
        from scitadel.secrets import store_api_key

        chat = self.query_one("#chat-messages", ChatMessageList)
        inp = self.query_one("#chat-input", Input)

        if not key.startswith(("sk-ant-", "sk-")):
            chat.add_system_message(
                "That doesn't look like an Anthropic API key. "
                "Keys start with sk-ant-..."
            )
            inp.focus()
            return

        store_api_key(key)
        self._awaiting_api_key = False
        inp.password = False
        inp.placeholder = "Describe your research topic..."

        # Force the engine to create a new client with the stored key
        self._engine._client = None

        chat.add_system_message("API key saved to keychain.")

        # Retry the pending message if there was one
        if self._retry_message:
            msg = self._retry_message
            self._retry_message = None
            self._busy = True
            self.run_worker(self._process_message(msg), thread=False)

    def open_question(self, question_id: str) -> None:
        """Load a research question into the chat and start working on it."""
        if self._busy:
            return
        store = self.app.store
        question = store.get_question(question_id)
        if not question:
            return

        chat = self.query_one("#chat-messages", ChatMessageList)
        terms = store.get_terms(question_id)
        terms_bar = self.query_one(TermsBar)

        # Show the question in chat
        prompt = f"Continue research on this question: {question.text}"
        if question.description:
            prompt += f"\nContext: {question.description}"
        if terms:
            all_terms = []
            for t in terms:
                all_terms.extend(t.terms)
            if all_terms:
                prompt += f"\nExisting search terms: {', '.join(all_terms)}"
                terms_bar.add_terms(all_terms)

        chat.add_user_message(prompt)
        self._busy = True
        self.run_worker(self._process_message(prompt), thread=False)

    def _handle_event(self, event: ChatEvent, chat: ChatMessageList) -> None:
        """Dispatch a single chat event to the appropriate widget."""
        if isinstance(event, TextDelta):
            if self._current_assistant_msg is None:
                self._current_assistant_msg = chat.add_assistant_message()
            self._current_assistant_msg.append_text(event.text)

        elif isinstance(event, ToolCallStart):
            self._current_assistant_msg = None
            indicator = chat.add_tool_indicator(event.tool_name, event.tool_id)
            self._tool_indicators[event.tool_id] = indicator

        elif isinstance(event, ToolCallResult):
            indicator = self._tool_indicators.pop(event.tool_id, None)
            if indicator:
                summary = _summarize_tool_result(event.tool_name, event.result)
                indicator.mark_complete(summary)
            self._process_tool_side_effects(event)

        elif isinstance(event, TurnComplete):
            self._current_assistant_msg = None

    def _process_tool_side_effects(self, event: ToolCallResult) -> None:
        """Update terms bar and results table based on tool results."""
        terms_bar = self.query_one(TermsBar)
        results_table = self.query_one(ResultsTable)

        if event.tool_name == "search":
            self._populate_papers_from_search(event.result, results_table)

        elif event.tool_name == "add_search_terms":
            self._extract_terms(event, terms_bar)

        elif event.tool_name == "create_question":
            pass  # Question created, no side effect on widgets

        elif event.tool_name == "assess_paper":
            self._update_paper_score(event.result, results_table)

    def _populate_papers_from_search(self, result: str, table: ResultsTable) -> None:
        """After a search, populate the results table from the store."""
        # Extract search ID from result
        for line in result.split("\n"):
            if line.startswith("Search ID:"):
                search_id = line.split(":", 1)[1].strip()
                papers = self.app.store.get_papers_for_search(search_id)
                for paper in papers:
                    table.add_paper(paper)
                return

    def _extract_terms(self, event: ToolCallResult, bar: TermsBar) -> None:
        """Extract terms from add_search_terms result."""
        # Result format: "Search terms added to question ...: ['term1', ...]"
        result = event.result
        if ":" in result:
            tail = result.rsplit(":", 1)[1].strip()
            try:
                terms = json.loads(tail.replace("'", '"'))
                if isinstance(terms, list):
                    bar.add_terms(terms)
            except (json.JSONDecodeError, ValueError):
                pass

    def _update_paper_score(self, result: str, table: ResultsTable) -> None:
        """Extract score from assess_paper result and update table."""
        paper_id = None
        score = None
        for line in result.split("\n"):
            if line.startswith("Score:"):
                try:
                    score = float(line.split(":", 1)[1].strip())
                except ValueError:
                    pass
        # We need the paper_id — look for it in the store assessments
        # The result contains the assessment ID, not paper_id directly
        # Parse from the result text
        for line in result.split("\n"):
            if line.startswith("Paper:"):
                # Find paper by title prefix in our table
                title_prefix = line.split(":", 1)[1].strip()
                for pid, row in table._papers.items():
                    if row.paper.title.startswith(title_prefix):
                        paper_id = pid
                        break
                break

        if paper_id and score is not None:
            table.update_score(paper_id, score)


def _summarize_tool_result(tool_name: str, result: str) -> str:
    """Create a short summary of a tool result."""
    if tool_name == "search":
        for line in result.split("\n"):
            if "Unique papers" in line:
                return line.strip()
        return "done"
    if tool_name == "assess_paper":
        for line in result.split("\n"):
            if line.startswith("Score:"):
                return line.strip()
        return "scored"
    if tool_name == "create_question":
        for line in result.split("\n"):
            if line.startswith("Question created"):
                return line.strip()
        return "created"
    if tool_name == "add_search_terms":
        return "terms added"
    if tool_name == "snowball_search":
        for line in result.split("\n"):
            if "New papers" in line:
                return line.strip()
        return "done"
    # Default: first line, truncated
    first_line = result.split("\n")[0] if result else "done"
    return first_line[:60]


def _friendly_api_error(exc: Exception) -> str:
    """Extract a human-readable message from an API error."""
    msg = str(exc)
    # Anthropic errors embed JSON — extract the message field
    if "'message':" in msg:
        try:
            import re

            match = re.search(r"'message':\s*'([^']+)'", msg)
            if match:
                return match.group(1)
        except Exception:
            pass
    # Truncate long error strings
    if len(msg) > 200:
        msg = msg[:200] + "..."
    return msg
