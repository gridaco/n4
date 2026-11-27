"""Edit N4's split project documents and query the native text layout."""

import fcntl
import json
import os
from pathlib import Path
import stat
import subprocess
import tempfile
from xml.parsers.expat import ParserCreate

from .xml import encode_sequence


class NativeError(ValueError):
    """The native reader or text engine rejected the requested operation."""


class ConcurrentEditError(RuntimeError):
    """The detail on disk has changed since this editor read it."""


def label(name, text, *, font, size, width, rgb=(0, 0, 0)):
    """Build a native graphic suitable for Sequence.add or a recipe's return value."""
    return {
        "name": name,
        "content": {"type": "label", "text": text, "font": os.fspath(font),
                    "size": size, "width": width, "rgb": list(rgb)},
    }


class Project:
    """A split project whose master and native validator determine its editable details."""

    @classmethod
    def open(cls, path, *, n4=None):
        entry = Path(path).resolve()
        if entry.is_dir():
            entry = entry / "project.proj"
        if entry.name != "project.proj":
            raise ValueError(f"Expected a split project directory or project.proj: {entry}")
        return cls(entry, n4 or os.environ.get("N4_BIN", "n4"))

    def __init__(self, entry, binary):
        self.entry = entry
        self._binary = os.fspath(binary)
        index = self.validate()
        self.details = tuple(index["details"])
        self._files = index["detail_files"]

    def _run(self, command, *options, candidate=None):
        try:
            result = subprocess.run([self._binary, command, self.entry, *options],
                                    input=candidate, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        except FileNotFoundError as error:
            raise FileNotFoundError(
                f"N4 executable {self._binary!r} was not found. Set N4_BIN or pass n4= to Project.open."
            ) from error
        if result.returncode:
            problem = result.stderr.decode(errors="replace").strip()
            raise NativeError(f"{self.entry}: {problem or f'n4 {command} exited {result.returncode}'}")
        return json.loads(result.stdout)

    def validate(self):
        """Validate the saved project and return the native detail index."""
        return self._run("validate")

    def sequence(self, detail_id):
        if detail_id not in self.details:
            raise KeyError(f"{self.entry}: no placed detail {detail_id!r}")
        return Sequence(self, detail_id)


class Sequence:
    """An independently editable in-memory snapshot of one placed detail."""

    def __init__(self, project, detail_id):
        self.project = project
        self.id = detail_id
        self.path = project.entry.parent / project._files[detail_id]
        self._accepted = self.path.read_bytes()
        if self.path.suffix == ".xml":
            self.document = project._run("read-detail", "--detail", detail_id, "--stdin",
                                         candidate=self._accepted)
        else:
            self.document = json.loads(self._accepted)
        self._accepted_encoding = self._encode()

    def _encode(self):
        if self.path.suffix == ".xml":
            return encode_sequence(self.document)
        return (json.dumps(self.document, indent=2, ensure_ascii=False, allow_nan=False) + "\n").encode()

    def layer(self, *names):
        """Find exactly one named child at each level and return its editable mapping."""
        if not names:
            raise ValueError("Supply a layer name")
        children = self.document.get("graphics", [])
        for index, name in enumerate(names):
            matches = [child for child in children if child.get("name") == name]
            if len(matches) != 1:
                path = " / ".join(names[:index + 1])
                raise KeyError(f"{self.path}: {path!r} matched {len(matches)} layers")
            item, = matches
            children = item["content"].get("children", [])
        return item

    def add(self, graphic, *parent):
        """Append a graphic at the root or below a named native group."""
        if parent:
            content = self.layer(*parent)["content"]
            if content["type"] not in ("group", "transform_group"):
                raise ValueError(f"{self.path}: {' / '.join(parent)} is not a group")
            content["children"].append(graphic)
        else:
            self.document.setdefault("graphics", []).append(graphic)

    def validate(self):
        """Validate the pending detail in the context of the saved master and its assets."""
        return self.project._run("validate", "--detail", self.id, candidate=self._encode())

    def metrics(self, *names):
        """Read native paragraph metrics for a named layer in the pending document."""
        if not names:
            raise ValueError("Supply a text layer name")
        options = [part for name in names for part in ("--layer", name)]
        return self.project._run("text-metrics", "--detail", self.id, *options, "--stdin",
                                 candidate=self._encode())

    def _check_revision(self):
        try:
            matches = self.path.read_bytes() == self._accepted
        except FileNotFoundError:
            matches = False
        if not matches:
            raise ConcurrentEditError(f"{self.path} changed on disk. Reopen the detail before saving.")

    def _preserve_markup(self, candidate):
        """Apply unambiguous single-line edits without touching comments or PIs."""
        protected = set()
        parser = ParserCreate()

        def remember(*parts):
            start = parser.CurrentLineNumber - 1
            protected.update(range(start, start + sum(part.count("\n") for part in parts) + 1))

        parser.CommentHandler = parser.ProcessingInstructionHandler = remember
        parser.Parse(self._accepted, True)
        if not protected:
            return candidate

        error = f"{self.path}: cannot place this edit while preserving XML comments or processing instructions"
        before = self._accepted_encoding.splitlines()
        after = candidate.splitlines()
        if len(before) != len(after):
            raise ValueError(error)
        lines = self._accepted.splitlines(keepends=True)
        originals = [line.strip() for line in lines]
        for old, new in zip(before, after):
            if old == new:
                continue
            matches = [i for i, text in enumerate(originals) if i not in protected and text == old.strip()]
            if len(matches) != 1:
                raise ValueError(error)
            index = matches[0]
            lines[index] = lines[index].replace(old.strip(), new.strip(), 1)
        patched = b"".join(lines)
        actual = self.project._run("read-detail", "--detail", self.id, "--stdin", candidate=patched)
        if actual != self.document:
            raise ValueError(error)
        return patched

    def write(self):
        """Validate and atomically publish this detail. Return whether the file changed."""
        candidate = self._encode()
        # Lock the stable directory inode. Replacing a detail changes its inode.
        directory = os.open(self.path.parent, os.O_RDONLY)
        try:
            fcntl.flock(directory, fcntl.LOCK_EX)
            self._check_revision()
            if candidate == self._accepted_encoding:
                return False
            encoding = candidate
            if self.path.suffix == ".xml" and self._accepted != self._accepted_encoding:
                candidate = self._preserve_markup(candidate)
            self.project._run("validate", "--detail", self.id, candidate=candidate)
            mode = stat.S_IMODE(self.path.stat().st_mode)
            with tempfile.NamedTemporaryFile(dir=self.path.parent, prefix=f".{self.id}-",
                                             delete=False) as stage:
                staged = Path(stage.name)
                try:
                    stage.write(candidate)
                    stage.flush()
                    os.fchmod(stage.fileno(), mode)
                    os.fsync(stage.fileno())
                    self._check_revision()
                    os.replace(staged, self.path)
                finally:
                    staged.unlink(missing_ok=True)
            self._accepted, self._accepted_encoding = candidate, encoding
            return True
        finally:
            os.close(directory)
