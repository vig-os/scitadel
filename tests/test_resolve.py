"""Tests for prefix resolution helper."""

from dataclasses import dataclass

from scitadel.services.resolve import resolve_prefix


@dataclass
class FakeEntity:
    id: str
    name: str


class TestResolvePrefix:
    def test_exact_match(self):
        items = [FakeEntity(id="abc123", name="A"), FakeEntity(id="def456", name="B")]
        result = resolve_prefix(items, "abc123", lambda x: x.id)
        assert result is not None
        assert result.id == "abc123"

    def test_prefix_match_unique(self):
        items = [FakeEntity(id="abc123", name="A"), FakeEntity(id="def456", name="B")]
        result = resolve_prefix(items, "abc", lambda x: x.id)
        assert result is not None
        assert result.id == "abc123"

    def test_prefix_match_ambiguous(self):
        items = [FakeEntity(id="abc123", name="A"), FakeEntity(id="abc456", name="B")]
        result = resolve_prefix(items, "abc", lambda x: x.id)
        assert result is None

    def test_no_match(self):
        items = [FakeEntity(id="abc123", name="A")]
        result = resolve_prefix(items, "xyz", lambda x: x.id)
        assert result is None

    def test_empty_list(self):
        result = resolve_prefix([], "abc", lambda x: x.id)
        assert result is None

    def test_exact_match_takes_priority(self):
        items = [
            FakeEntity(id="abc", name="Short"),
            FakeEntity(id="abc123", name="Long"),
        ]
        result = resolve_prefix(items, "abc", lambda x: x.id)
        assert result is not None
        assert result.name == "Short"
