"""Tests for configuration loading."""

from scitadel.config import Config, SourceConfig, load_config


class TestConfig:
    def test_default_config(self):
        config = Config()
        assert config.db_path.name == "scitadel.db"
        assert "pubmed" in config.default_sources
        assert "arxiv" in config.default_sources

    def test_source_config_defaults(self):
        sc = SourceConfig()
        assert sc.enabled is True
        assert sc.timeout == 30.0
        assert sc.max_retries == 3

    def test_load_config(self):
        config = load_config()
        assert isinstance(config, Config)
        assert config.db_path.is_absolute()
