#!/usr/bin/env python3
"""Convert a saved benchmark report into Markdown tables."""

from __future__ import annotations

import argparse
import pathlib
import re
import sys


SECTION_HEADER_RE = re.compile(r"^=== (?P<label>.+) ===$")
KEY_VALUE_RE = re.compile(r"(?P<key>[a-zA-Z0-9_]+)=(?P<value>[^\s]+)")


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Convert a benchmark report into Markdown tables"
    )
    parser.add_argument("report", type=pathlib.Path, help="Path to benchmark report")
    parser.add_argument(
        "--output",
        type=pathlib.Path,
        help="Optional Markdown output path; prints to stdout when omitted",
    )
    args = parser.parse_args()

    markdown = render_markdown(parse_report(args.report))

    if args.output is not None:
        args.output.write_text(markdown, encoding="utf-8")
    else:
        sys.stdout.write(markdown)

    return 0


def parse_report(report_path: pathlib.Path) -> dict:
    lines = report_path.read_text(encoding="utf-8").splitlines()

    metadata: dict[str, str] = {}
    sections: list[dict] = []
    current_section: dict | None = None
    current_case: dict | None = None

    for raw_line in lines:
        line = raw_line.strip()
        if not line:
            continue

        header_match = SECTION_HEADER_RE.match(line)
        if header_match:
            current_section = {
                "label": header_match.group("label"),
                "cases": [],
                "command": None,
            }
            sections.append(current_section)
            current_case = None
            continue

        if current_section is None:
            if "=" in line:
                key, value = line.split("=", 1)
                metadata[key] = value
            continue

        if line.startswith("command:"):
            current_section["command"] = line.removeprefix("command:").strip()
            continue

        if line.startswith("benchmark "):
            current_case = {
                "benchmark": parse_key_values(line),
                "median": {},
                "resource": {},
            }
            current_section["cases"].append(current_case)
            continue

        if line.startswith("median "):
            if current_case is None:
                raise ValueError(f"median line without benchmark case in {report_path}")
            current_case["median"] = parse_key_values(line)
            continue

        if line.startswith("resource "):
            resource_values = parse_key_values(line)
            for case in current_section["cases"]:
                if not case["resource"]:
                    case["resource"] = resource_values
            continue

    return {"report_path": report_path, "metadata": metadata, "sections": sections}


def parse_key_values(line: str) -> dict[str, str]:
    return {
        match.group("key"): match.group("value")
        for match in KEY_VALUE_RE.finditer(line)
    }


def render_markdown(parsed: dict) -> str:
    lines = [f"# Benchmark Report", ""]
    lines.append(f"Source: `{parsed['report_path']}`")
    lines.append("")

    metadata = parsed["metadata"]
    if metadata:
        lines.extend(
            [
                "## Environment",
                "",
                "| Key | Value |",
                "| --- | --- |",
            ]
        )
        for key, value in metadata.items():
            lines.append(f"| `{key}` | `{value}` |")
        lines.append("")

    lines.extend(
        [
            "## Summary",
            "",
            "| Section | Mode | Detection | Output | Runs | Frames | End-to-end ms/frame | End-to-end fps | Detector ms/frame | Detector fps | Max RSS MB | User CPU s | Sys CPU s |",
            "| --- | --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |",
        ]
    )

    for section in parsed["sections"]:
        for case in section["cases"]:
            benchmark = case["benchmark"]
            median = case["median"]
            resource = case["resource"]
            lines.append(
                "| {section} | `{mode}` | `{detect}` | `{output}` | {runs} | {frames} | {wall_ms_per_frame} | {wall_fps} | {analyze_ms_per_frame} | {analyze_fps} | {rss_mb} | {user_cpu_s} | {sys_cpu_s} |".format(
                    section=escape_pipes(section["label"]),
                    mode=benchmark.get("mode", "?"),
                    detect=benchmark.get("detection", "?"),
                    output=benchmark.get("output", "?"),
                    runs=benchmark.get("runs", "?"),
                    frames=median.get("frames", "?"),
                    wall_ms_per_frame=median.get("wall_ms_per_frame", "?"),
                    wall_fps=median.get("wall_fps", "?"),
                    analyze_ms_per_frame=median.get("analyze_ms_per_frame", "?"),
                    analyze_fps=median.get("analyze_fps", "?"),
                    rss_mb=format_rss_mb(resource.get("max_rss_kb")),
                    user_cpu_s=resource.get("user_cpu_s", "?"),
                    sys_cpu_s=resource.get("sys_cpu_s", "?"),
                )
            )

    lines.append("")
    lines.append("## Commands")
    lines.append("")
    for section in parsed["sections"]:
        if not section.get("command"):
            continue
        lines.append(f"### {section['label']}")
        lines.append("")
        lines.append("```bash")
        lines.append(section["command"])
        lines.append("```")
        lines.append("")

    return "\n".join(lines)


def escape_pipes(value: str) -> str:
    return value.replace("|", "\\|")


def format_rss_mb(raw_kb: str | None) -> str:
    if raw_kb is None:
        return "?"

    try:
        rss_mb = int(raw_kb) / 1024.0
    except ValueError:
        return raw_kb

    return f"{rss_mb:.1f}"


if __name__ == "__main__":
    raise SystemExit(main())
