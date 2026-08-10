from __future__ import annotations

from pathlib import Path

from cce_research.adapters import Adapter


def test_adapter_resolves_workspace_executable_before_repository_chdir(
    tmp_path: Path, monkeypatch
) -> None:
    executable = tmp_path / "target" / "release" / "cce"
    executable.parent.mkdir(parents=True)
    executable.write_text("binary", encoding="utf-8")
    adapter_file = tmp_path / "adapter.yaml"
    adapter_file.write_text(
        "name: cce\ncommand: [target/release/cce, --json]\n",
        encoding="utf-8",
    )
    monkeypatch.chdir(tmp_path)

    adapter = Adapter.load(adapter_file)

    assert adapter.command[0] == str(executable.resolve())
