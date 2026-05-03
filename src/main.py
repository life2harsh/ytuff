#!/usr/bin/env python3

from pathlib import Path
import argparse

DEFAULT_IGNORE_DIRS = {
    ".git",
    ".hg",
    ".svn",
    "__pycache__",
    ".pytest_cache",
    ".mypy_cache",
    ".ruff_cache",
    ".idea",
    ".vscode",
    "node_modules",
    "dist",
    "build",
    ".next",
    ".nuxt",
    ".venv",
    "venv",
    "env",
    ".env",
    "coverage",
}

DEFAULT_IGNORE_FILES = {
    "package-lock.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "poetry.lock",
    "uv.lock",
}

DEFAULT_SOURCE_EXTS = {
    ".py",
    ".js",
    ".jsx",
    ".ts",
    ".tsx",
    ".java",
    ".c",
    ".h",
    ".cpp",
    ".hpp",
    ".cs",
    ".go",
    ".rs",
    ".php",
    ".rb",
    ".swift",
    ".kt",
    ".kts",
    ".scala",
    ".sh",
    ".bash",
    ".zsh",
    ".ps1",
    ".html",
    ".css",
    ".scss",
    ".sass",
    ".json",
    ".yaml",
    ".yml",
    ".toml",
    ".xml",
    ".sql",
    ".md",
    ".txt",
    ".dockerfile",
}

LANG_BY_EXT = {
    ".py": "python",
    ".js": "javascript",
    ".jsx": "jsx",
    ".ts": "typescript",
    ".tsx": "tsx",
    ".java": "java",
    ".c": "c",
    ".h": "c",
    ".cpp": "cpp",
    ".hpp": "cpp",
    ".cs": "csharp",
    ".go": "go",
    ".rs": "rust",
    ".php": "php",
    ".rb": "ruby",
    ".swift": "swift",
    ".kt": "kotlin",
    ".kts": "kotlin",
    ".scala": "scala",
    ".sh": "bash",
    ".bash": "bash",
    ".zsh": "zsh",
    ".ps1": "powershell",
    ".html": "html",
    ".css": "css",
    ".scss": "scss",
    ".sass": "sass",
    ".json": "json",
    ".yaml": "yaml",
    ".yml": "yaml",
    ".toml": "toml",
    ".xml": "xml",
    ".sql": "sql",
    ".md": "markdown",
    ".txt": "",
}


def is_probably_binary(path: Path) -> bool:
    try:
        with path.open("rb") as f:
            chunk = f.read(4096)
        return b"\0" in chunk
    except OSError:
        return True


def should_ignore(path: Path, root: Path, ignore_dirs: set[str], ignore_files: set[str]) -> bool:
    rel_parts = path.relative_to(root).parts

    if any(part in ignore_dirs for part in rel_parts):
        return True

    if path.name in ignore_files:
        return True

    return False


def collect_files(root: Path, source_exts: set[str], ignore_dirs: set[str], ignore_files: set[str]) -> list[Path]:
    files = []

    for path in root.rglob("*"):
        if should_ignore(path, root, ignore_dirs, ignore_files):
            continue

        if not path.is_file():
            continue

        suffix = path.suffix.lower()

        # Special case for Dockerfile
        if path.name == "Dockerfile":
            files.append(path)
            continue

        if suffix not in source_exts:
            continue

        if is_probably_binary(path):
            continue

        files.append(path)

    return sorted(files, key=lambda p: str(p.relative_to(root)).lower())


def build_tree(root: Path, files: list[Path]) -> str:
    tree = {}

    for file in files:
        current = tree
        for part in file.relative_to(root).parts:
            current = current.setdefault(part, {})

    lines = [root.name + "/"]

    def walk(node: dict, prefix: str = ""):
        items = sorted(node.items(), key=lambda item: (
            bool(item[1]), item[0].lower()))

        for index, (name, children) in enumerate(items):
            is_last = index == len(items) - 1
            branch = "└── " if is_last else "├── "
            lines.append(prefix + branch + name)

            if children:
                extension = "    " if is_last else "│   "
                walk(children, prefix + extension)

    walk(tree)
    return "\n".join(lines)


def fence_content(content: str) -> tuple[str, str]:
    """
    Use a longer fence if the file content already contains triple backticks.
    """
    fence = "```"
    while fence in content:
        fence += "`"
    return fence, content


def read_text_file(path: Path) -> str:
    encodings = ["utf-8", "utf-8-sig", "latin-1"]

    for encoding in encodings:
        try:
            return path.read_text(encoding=encoding)
        except UnicodeDecodeError:
            continue

    raise UnicodeDecodeError("unknown", b"", 0, 1, "Could not decode file")


def write_markdown(root: Path, files: list[Path], output: Path) -> None:
    with output.open("w", encoding="utf-8", newline="\n") as out:
        out.write("# Project Source Dump\n\n")

        out.write("## File Tree\n\n")
        out.write("```text\n")
        out.write(build_tree(root, files))
        out.write("\n```\n\n")

        out.write("## Files\n\n")

        for file in files:
            rel_path = "./" + file.relative_to(root).as_posix()

            try:
                content = read_text_file(file)
            except Exception as exc:
                content = f"[Could not read file: {exc}]"

            lang = LANG_BY_EXT.get(file.suffix.lower(), "")
            if file.name == "Dockerfile":
                lang = "dockerfile"

            fence, content = fence_content(content)

            out.write(f"{rel_path}\n")
            out.write(f"{fence}{lang}\n")
            out.write(content)
            if not content.endswith("\n"):
                out.write("\n")
            out.write(f"{fence}\n\n")


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Recursively dump project source files into a Markdown file."
    )

    parser.add_argument(
        "project_dir",
        nargs="?",
        default=".",
        help="Project directory to scan. Defaults to current directory.",
    )

    parser.add_argument(
        "-o",
        "--output",
        default="project_source_dump.md",
        help="Output Markdown file. Defaults to project_source_dump.md.",
    )

    parser.add_argument(
        "--include-ext",
        action="append",
        default=[],
        help="Extra extension to include, for example: --include-ext .env",
    )

    parser.add_argument(
        "--ignore-dir",
        action="append",
        default=[],
        help="Extra directory name to ignore, for example: --ignore-dir logs",
    )

    parser.add_argument(
        "--ignore-file",
        action="append",
        default=[],
        help="Extra file name to ignore, for example: --ignore-file secrets.txt",
    )

    args = parser.parse_args()

    root = Path(args.project_dir).resolve()
    output = Path(args.output).resolve()

    if not root.exists() or not root.is_dir():
        raise SystemExit(
            f"Project directory does not exist or is not a directory: {root}")

    source_exts = DEFAULT_SOURCE_EXTS | {ext if ext.startswith(
        ".") else "." + ext for ext in args.include_ext}
    ignore_dirs = DEFAULT_IGNORE_DIRS | set(args.ignore_dir)
    ignore_files = DEFAULT_IGNORE_FILES | set(args.ignore_file)

    files = collect_files(root, source_exts, ignore_dirs, ignore_files)

    # Avoid including the generated output file if it is inside the project.
    files = [file for file in files if file.resolve() != output]

    write_markdown(root, files, output)

    print(f"Wrote {len(files)} files to {output}")


if __name__ == "__main__":
    main()
