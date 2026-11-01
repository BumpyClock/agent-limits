#!/usr/bin/env python3
"""Exercise executable hooks/installers in disposable Git repositories. No push."""
import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

SOURCE = Path(__file__).resolve().parents[2]


class HookTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="hooks space ")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.env = dict(os.environ, GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull)
        self.run_command("git", "init", "-q")
        shutil.copytree(SOURCE / ".githooks", self.root / ".githooks")
        (self.root / "scripts/hooks").mkdir(parents=True)
        shutil.copy2(SOURCE / "scripts/install-hooks.sh", self.root / "scripts/install-hooks.sh")
        # Stub only the project payload. Range/clean-tree/installer code is real.
        (self.root / "scripts/hooks/check.sh").write_text(
            '#!/usr/bin/env bash\nprintf "checked\\n" >> "$HOOK_RECEIPT"\nexit "${HOOK_EXIT:-0}"\n')
        self.env["HOOK_RECEIPT"] = str(self.root / ".git/receipt")
        self.commit()
        self.base = self.git("rev-parse", "HEAD").stdout.strip()

    def run_command(self, *args, input=None, expected=0):
        result = subprocess.run(args, cwd=self.root, env=self.env, input=input,
                                text=True, capture_output=True)
        if expected is not None:
            self.assertEqual(result.returncode, expected, result.stdout + result.stderr)
        return result

    def git(self, *args, **kwargs):
        return self.run_command("git", *args, **kwargs)

    def commit(self):
        self.git("add", ".")
        self.git("-c", "user.name=Hook Fixture", "-c", "user.email=fixture@invalid",
                 "-c", "commit.gpgsign=false", "-c", "core.hooksPath=/dev/null",
                 "commit", "-qm", "fixture", "--allow-empty")

    def update(self, path="source with spaces.go"):
        (self.root / path).write_text("changed\n")
        self.commit()
        return self.git("rev-parse", "HEAD").stdout.strip()

    def row(self, local=None, remote=None):
        head = self.git("rev-parse", "HEAD").stdout.strip()
        return f"refs/heads/test {local or head} refs/heads/test {remote or self.base}\n"

    def hook(self, data=None, expected=0):
        return self.run_command(str(self.root / ".githooks/pre-push"), "origin", "unused",
                                input=self.row() if data is None else data, expected=expected)

    def checked(self):
        return (self.root / ".git/receipt").exists()

    def test_changed_path_with_spaces_and_failure_propagation(self):
        self.update()
        self.env["HOOK_EXIT"] = "23"
        self.hook(expected=23)
        self.assertTrue(self.checked())

    def test_new_branch_unknown_base_and_empty_input_broaden(self):
        for data in (self.row(remote="0" * 40), self.row(remote="1" * 40), "", self.row()):
            self.hook(data)
            self.assertTrue(self.checked())
            (self.root / ".git/receipt").unlink()

    def test_deleting_ref_needs_no_project_tools(self):
        self.hook(self.row(local="0" * 40))
        self.assertFalse(self.checked())

    def test_multiple_refs_cannot_hide_later_failure(self):
        self.update()
        self.env["HOOK_EXIT"] = "17"
        self.hook(self.row(local="0" * 40) + self.row(), expected=17)
        self.assertTrue(self.checked())

    def test_last_record_without_newline_is_not_skipped(self):
        self.update()
        self.env["HOOK_EXIT"] = "19"
        self.hook(self.row(local="0" * 40) + self.row().rstrip("\n"), expected=19)
        self.assertTrue(self.checked())

    def test_git_status_failure_is_not_green(self):
        self.update()
        real_git = shutil.which("git")
        tools = self.root / ".git/tools"
        tools.mkdir()
        stub = tools / "git"
        stub.write_text(f'#!/bin/sh\nif [ "$1" = status ]; then exit 42; fi\nexec "{real_git}" "$@"\n')
        stub.chmod(0o755)
        self.env["PATH"] = str(tools) + os.pathsep + self.env["PATH"]
        self.hook(expected=42)
        self.assertFalse(self.checked())

    def test_non_head_and_malformed_input_fail(self):
        self.update()
        self.hook(self.row(local=self.base), expected=1)
        self.hook("malformed\n", expected=1)
        self.assertFalse(self.checked())

    def test_dirty_checkout_refused_without_rewriting(self):
        self.update()
        target = self.root / "untracked.txt"
        target.write_text("preserve me")
        self.hook(expected=1)
        self.assertEqual(target.read_text(), "preserve me")
        self.assertFalse(self.checked())

    def test_rename_and_deletion_not_skipped(self):
        self.update()
        self.git("mv", "source with spaces.go", "renamed.go")
        self.commit()
        self.hook()
        self.assertTrue(self.checked())
        (self.root / ".git/receipt").unlink()
        self.git("rm", "renamed.go")
        self.commit()
        self.hook()
        self.assertTrue(self.checked())

    def test_installer_idempotent_and_preserves_active_hook(self):
        active = self.root / ".git/hooks/pre-commit"
        active.write_text("#!/bin/sh\nexit 7\n")
        active.chmod(0o755)
        self.run_command("bash", "scripts/install-hooks.sh", expected=1)
        self.assertEqual(active.read_text(), "#!/bin/sh\nexit 7\n")
        active.unlink()
        self.run_command("bash", "scripts/install-hooks.sh")
        self.run_command("bash", "scripts/install-hooks.sh")
        self.assertEqual(self.git("config", "--get", "core.hooksPath").stdout.strip(), ".githooks")

    def test_installer_preserves_custom_configuration(self):
        self.git("config", "core.hooksPath", "custom hooks")
        self.run_command("bash", "scripts/install-hooks.sh", expected=1)
        self.assertEqual(self.git("config", "--get", "core.hooksPath").stdout.strip(), "custom hooks")

    def test_linked_worktree_installer_resolves_its_own_checkout(self):
        with tempfile.TemporaryDirectory(prefix="linked hooks ") as directory:
            linked = Path(directory) / "checkout"
            self.git("worktree", "add", "--detach", str(linked), "HEAD")
            result = subprocess.run(["bash", "scripts/install-hooks.sh"], cwd=linked,
                                    env=self.env, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            result = subprocess.run([str(linked / ".githooks/pre-push")], cwd=linked,
                                    env=self.env, input="", capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertTrue(self.checked())

    def test_content_only_skip_and_instruction_fallback(self):
        if not (SOURCE / "package.json").exists() or 'personal-assistant' not in (SOURCE / "package.json").read_text():
            self.skipTest("PersonalAssistant content policy only")
        self.update("notes.md")
        self.hook()
        self.assertFalse(self.checked())
        self.update("AGENTS.md")
        self.hook()
        self.assertTrue(self.checked())


if __name__ == "__main__":
    unittest.main()
