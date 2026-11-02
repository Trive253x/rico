#!/usr/bin/env python3
"""Riassume i risultati dei controlli di sicurezza in un rapporto Markdown.

Legge dalla cartella indicata:
  audit.json     cargo audit --json
  deny.jsonl     cargo deny --format json (una diagnostica per riga)
  semgrep.json   semgrep --json
  gitleaks.json  gitleaks --report-format json --redact

Sono "importanti" (fanno partire la notifica):
  - vulnerabilità note nelle dipendenze (cargo audit);
  - risultati Semgrep di gravità ERROR;
  - qualsiasi segreto trovato da Gitleaks;
  - un controllo che non è riuscito a girare (meglio un falso allarme che un
    controllo saltato in silenzio).
Il resto (dipendenze non più mantenute, licenze, avvisi minori) finisce nel
rapporto ma non fa scattare la notifica.

Scrive `importanti=true|false` in $GITHUB_OUTPUT e il rapporto su stdout.
"""

import json
import os
import sys
from pathlib import Path


def load_json(path: Path):
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None


def esc(text) -> str:
    """Testo sicuro dentro una tabella Markdown."""
    return str(text).replace("|", "\\|").replace("\n", " ").strip()


def audit(report: Path, important: list, minor: list, problems: list):
    data = load_json(report / "audit.json")
    if data is None:
        problems.append("cargo audit non ha prodotto risultati")
        return
    for v in data.get("vulnerabilities", {}).get("list", []):
        adv = v.get("advisory", {})
        pkg = v.get("package", {})
        patched = ", ".join(v.get("versions", {}).get("patched", [])) or "nessuna"
        important.append(
            (
                "cargo audit",
                f"{adv.get('id', '?')}: {adv.get('title', '')}",
                f"{pkg.get('name', '?')} {pkg.get('version', '')} (corretta in: {patched})",
                adv.get("url") or f"https://rustsec.org/advisories/{adv.get('id', '')}",
            )
        )
    for kind, items in (data.get("warnings") or {}).items():
        for w in items or []:
            adv = w.get("advisory") or {}
            pkg = w.get("package", {})
            minor.append(
                (
                    f"cargo audit ({kind})",
                    f"{adv.get('id', '')} {adv.get('title', '')}".strip() or kind,
                    f"{pkg.get('name', '?')} {pkg.get('version', '')}",
                    adv.get("url") or "",
                )
            )


def deny(report: Path, important: list, minor: list, problems: list):
    path = report / "deny.jsonl"
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except OSError:
        problems.append("cargo deny non ha prodotto risultati")
        return
    for line in lines:
        try:
            item = json.loads(line)
        except ValueError:
            continue
        if item.get("type") != "diagnostic":
            continue
        fields = item.get("fields", {})
        severity = fields.get("severity", "")
        if severity not in ("error", "warning"):
            continue
        labels = fields.get("labels") or [{}]
        where = labels[0].get("span", "") if labels else ""
        minor.append(
            (
                f"cargo deny ({severity})",
                fields.get("message", ""),
                where,
                "",
            )
        )


def semgrep(report: Path, important: list, minor: list, problems: list):
    data = load_json(report / "semgrep.json")
    if data is None:
        problems.append("Semgrep non ha prodotto risultati")
        return
    for r in data.get("results", []):
        extra = r.get("extra", {})
        severity = extra.get("severity", "")
        row = (
            f"Semgrep ({severity.lower()})",
            f"{r.get('check_id', '')}: {extra.get('message', '')}",
            f"{r.get('path', '')}:{r.get('start', {}).get('line', '')}",
            "",
        )
        (important if severity == "ERROR" else minor).append(row)
    for e in data.get("errors", []):
        if e.get("level") == "error":
            problems.append(f"Semgrep: {e.get('message', 'errore')}")


def gitleaks(report: Path, important: list, minor: list, problems: list):
    data = load_json(report / "gitleaks.json")
    if data is None:
        problems.append("Gitleaks non ha prodotto risultati")
        return
    for f in data:
        # Il valore del segreto non viene mai copiato nel rapporto.
        important.append(
            (
                "Gitleaks",
                f"{f.get('RuleID', '')}: {f.get('Description', '')}",
                f"{f.get('File', '')}:{f.get('StartLine', '')} (commit {str(f.get('Commit', ''))[:8]})",
                "",
            )
        )


def table(rows: list) -> str:
    out = ["| Strumento | Problema | Dove | Dettagli |", "|---|---|---|---|"]
    for tool, what, where, link in rows:
        details = f"[apri]({link})" if link else ""
        out.append(f"| {esc(tool)} | {esc(what)} | {esc(where)} | {details} |")
    return "\n".join(out)


def main() -> int:
    report = Path(sys.argv[1] if len(sys.argv) > 1 else "report")
    important, minor, problems = [], [], []
    for check in (audit, deny, semgrep, gitleaks):
        check(report, important, minor, problems)
    if os.environ.get("JOBS_OK", "true") != "true":
        problems.append("almeno un job di controllo è fallito")

    alarm = bool(important or problems)
    owner = os.environ.get("OWNER", "")
    ref = os.environ.get("REF", "")
    sha = os.environ.get("SHA", "")[:8]
    run_url = os.environ.get("RUN_URL", "")

    lines = ["## Controlli di sicurezza", ""]
    lines.append(f"Branch `{ref}`, commit `{sha}`." + (f" [Dettagli dell'esecuzione]({run_url})" if run_url else ""))
    lines.append("")
    if alarm:
        if owner:
            lines.append(f"@{owner} ci sono problemi da guardare.")
            lines.append("")
    else:
        lines.append("Nessuna falla importante trovata.")
        lines.append("")
    if important:
        lines += [f"### Falle importanti ({len(important)})", "", table(important), ""]
    if problems:
        lines += ["### Controlli non completati", ""] + [f"- {esc(p)}" for p in problems] + [""]
    if minor:
        lines += [
            f"<details><summary>Avvisi minori ({len(minor)})</summary>",
            "",
            table(minor),
            "",
            "</details>",
            "",
        ]
    print("\n".join(lines))

    output = os.environ.get("GITHUB_OUTPUT")
    if output:
        with open(output, "a", encoding="utf-8") as fh:
            fh.write(f"importanti={'true' if alarm else 'false'}\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
