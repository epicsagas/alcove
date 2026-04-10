<p align="center">
  <img src="../alcove.png" alt="Alcove" width="100%" />
</p>

<p align="center"><strong>Dein KI-Agent kennt dein Projekt nicht. Alcove ändert das.</strong></p>

<p align="center">
  <a href="../README.md">English</a> ·
  <a href="README.ko.md">한국어</a> ·
  <a href="README.ja.md">日本語</a> ·
  <a href="README.zh-CN.md">简体中文</a> ·
  <a href="README.es.md">Español</a> ·
  <a href="README.hi.md">हिन्दी</a> ·
  <a href="README.pt-BR.md">Português</a> ·
  <a href="README.de.md">Deutsch</a> ·
  <a href="README.fr.md">Français</a> ·
  <a href="README.ru.md">Русский</a>
</p>

<p align="center">
  <a href="https://crates.io/crates/alcove"><img src="https://img.shields.io/crates/v/alcove.svg" alt="crates.io" /></a>
  <a href="https://crates.io/crates/alcove"><img src="https://img.shields.io/crates/d/alcove.svg" alt="Downloads" /></a>
  <a href="../LICENSE"><img src="https://img.shields.io/badge/License-Apache%202.0-blue.svg" alt="License" /></a>
  <a href="https://buymeacoffee.com/epicsaga"><img src="https://img.shields.io/badge/Buy%20Me%20a%20Coffee-FFDD00?style=flat&logo=buy-me-a-coffee&logoColor=black" alt="Buy Me a Coffee" /></a>
</p>

Alcove ermöglicht jedem KI-Codierungs-Agenten, deine private Projektdokumentation zu lesen und verwalten — ohne sie in öffentliche Repositories zu leaken.

Speichere PRDs, Architekturentscheidungen, Secret-Maps und interne Runbooks an einem Ort. Jeder MCP-kompatible Agent erhält dieselben Tools, über alle Projekte hinweg, ohne Konfiguration pro Projekt.

## Das Problem

Du hast zwei schlechte Optionen.

**Option A: Docs in `CLAUDE.md` / `AGENTS.md` packen**
Bei jeder Ausführung wird jede Datei in das Kontextfenster injiziert.
Funktioniert für kurze Konventionen. Bricht bei echten Projektdokumenten zusammen.
10 Architekturdateien = Kontext-Aufblähung = langsamere, teurere, ungenauere Antworten.

**Option B: Keine Docs einfügen**
Dein Agent erfindet Anforderungen, die du bereits dokumentiert hast.
Er ignoriert Einschränkungen aus Entscheidungen, die du bereits getroffen hast.
Er bittet dich jede Session, dieselben Dinge zu erklären.

Keine Option skaliert. Multipliziere das mit 5 Projekten und 3 Agenten, jeder unterschiedlich konfiguriert. Jedes Mal wenn du wechselst, verlierst du den Kontext.

## Wie Alcove das löst

Alcove speichert alle deine privaten Dokumente in **einem gemeinsamen Repository**, organisiert nach Projekt. Jeder MCP-kompatible Agent greift auf dieselbe Weise darauf zu — egal ob du Claude Code, Cursor, Gemini CLI oder Codex verwendest.

```
~/projects/my-app $ claude "Wie ist die Authentifizierung implementiert?"

  → Alcove erkennt Projekt: my-app
  → Liest ~/documents/my-app/ARCHITECTURE.md
  → Agent antwortet mit echtem Projektkontext
```

```
~/projects/my-api $ codex "Überprüfe das API-Design"

  → Alcove erkennt Projekt: my-api
  → Gleiche Dokumentstruktur, gleiches Zugriffsmuster
  → Anderes Projekt, gleicher Workflow
```

**Wechsle den Agenten jederzeit. Wechsle das Projekt jederzeit. Die Dokumentschicht bleibt standardisiert.**

## Hauptfunktionen

- **Ein Docs-Repository, mehrere Projekte** — private Dokumente nach Projekt organisiert, an einem Ort verwaltet
- **Einmal einrichten, jeder Agent** — einmal konfigurieren, jeder MCP-kompatible Agent erhält dieselben Tools
- **Automatische Projekterkennung** vom CWD — keine Konfiguration pro Projekt nötig
- **Bereichsbezogener Zugriff** — jedes Projekt sieht nur seine eigenen Dokumente
- **Intelligente Suche** — BM25-Ranking-Suche mit automatischer Indexierung; findet die relevantesten Dokumente zuerst, fällt bei Bedarf auf grep zurück
- **Projektübergreifende Suche** — suche in allen Projekten gleichzeitig mit `scope: "global"` — nutze es als persönliche Wissensdatenbank
- **Private Dokumente bleiben privat** — sensible Dokumente (Secret-Map, interne Entscheidungen, technische Schulden) berühren nie dein öffentliches Repository
- **Standardisierte Dokumentstruktur** — `policy.toml` erzwingt konsistente Dokumente über alle Projekte und Teams
- **Cross-Repo-Audit** — findet fehlplatzierte interne Dokumente im Projektrepository und schlägt Korrekturen vor
- **Dokumentvalidierung** — prüft auf fehlende Dateien, unausgefüllte Templates, erforderliche Abschnitte
- **Funktioniert mit 9+ Agenten** — Claude Code, Cursor, Claude Desktop, Cline, OpenCode, Codex, Copilot, Antigravity, Gemini CLI

## Warum Alcove

| Ohne Alcove | Mit Alcove |
|-------------|------------|
| Interne Dokumente verstreut über Notion, Google Docs, lokale Dateien | Ein Docs-Repository, nach Projekt strukturiert |
| Jeder KI-Agent separat für Dokumentzugriff konfiguriert | Einmal einrichten, alle Agenten teilen dieselben Tools |
| Projektwechsel bedeutet Verlust des Dokumentkontexts | CWD-Autoerkennung, sofortiger Projektwechsel |
| Agentensuche liefert zufällige Treffer | BM25-Ranking-Suche — beste Treffer zuerst, automatische Indexierung |
| "Alle meine Notizen zur Authentifizierung durchsuchen" — unmöglich | Globale Suche über alle Projekte in einer Abfrage |
| Sensible Dokumente riskieren Leak in öffentliche Repos | Private Dokumente physisch von Projekt-Repos getrennt |
| Dokumentstruktur variiert pro Projekt und Teammitglied | `policy.toml` erzwingt Standards über alle Projekte |
| Keine Möglichkeit zu prüfen, ob Dokumente vollständig sind | `validate` erkennt fehlende Dateien, leere Templates, fehlende Abschnitte |

## Schnellstart

```bash
# macOS
brew install epicsagas/alcove/alcove

# Linux / Windows — vorgefertigtes Binary (schnell, keine Kompilierung)
cargo install cargo-binstall
cargo binstall alcove

# Beliebige Plattform — aus Quellcode
cargo install alcove

# Klonen und bauen
git clone https://github.com/epicsagas/alcove.git
cd alcove
make install

alcove setup
```

Das war's. `setup` führt dich interaktiv durch alles:

1. Wo deine Dokumente liegen
2. Welche Dokumentkategorien verfolgt werden sollen
3. Bevorzugtes Diagrammformat
4. Welche KI-Agenten konfiguriert werden sollen (MCP + Skill-Dateien)

Führe `alcove setup` jederzeit erneut aus, um Einstellungen zu ändern. Es merkt sich deine vorherigen Auswahlen.

## Funktionsweise

```mermaid
flowchart LR
    subgraph Projects["Deine Projekte"]
        A1["my-app/\n  src/ ..."]
        A2["my-api/\n  src/ ..."]
    end

    subgraph Docs["Deine privaten Dokumente (ein Repository)"]
        D1["my-app/\n  PRD.md\n  ARCH.md"]
        D2["my-api/\n  PRD.md\n  ..."]
        P1["policy.toml"]
    end

    subgraph Agents["Jeder MCP-Agent"]
        AG["Claude Code · Cursor\nGemini CLI · Codex · Copilot\n+4 more"]
    end

    subgraph MCP["Alcove MCP-Server"]
        T["search · get_file\noverview · audit\ninit · validate"]
    end

    A1 -- "CWD erkannt" --> D1
    A2 -- "CWD erkannt" --> D2
    Agents -- "stdio MCP" --> MCP
    MCP -- "bereichsbezogener Zugriff" --> Docs
```

Deine Dokumente sind in einem separaten Verzeichnis (`DOCS_ROOT`) organisiert, ein Ordner pro Projekt. Alcove verwaltet Dokumente dort und stellt sie jedem MCP-kompatiblen KI-Agenten über stdio bereit. Dein Agent ruft Tools wie `get_doc_file("PRD.md")` auf und erhält projektspezifische Antworten — unabhängig davon, welchen Agenten du verwendest.

## Dokumentklassifizierung

Alcove klassifiziert Dokumente in folgende Stufen:

| Klassifizierung | Ort | Beispiele |
|----------------|-----|-----------|
| **doc-repo-required** | Alcove (privat) | PRD, Architecture, Decisions, Conventions |
| **doc-repo-supplementary** | Alcove (privat) | Deployment, Onboarding, Testing, Runbook |
| **reference** | Alcove `reports/` Ordner | Audit-Berichte, Benchmarks, Analysen |
| **project-repo** | GitHub-Repository (öffentlich) | README, CHANGELOG, CONTRIBUTING |

Das `audit`-Tool scannt sowohl das Docs-Repository als auch das lokale Projektverzeichnis und schlägt Aktionen vor — wie das Generieren einer öffentlichen README aus deinem privaten PRD oder das Zurückholen fehlplatzierter Berichte nach Alcove.

## MCP-Tools

| Tool | Funktion |
|------|----------|
| `get_project_docs_overview` | Alle Dokumente mit Klassifizierung und Größen auflisten |
| `search_project_docs` | Intelligente Suche — wählt automatisch BM25-Ranking oder grep, unterstützt `scope: "global"` für projektübergreifende Suche |
| `get_doc_file` | Ein bestimmtes Dokument nach Pfad lesen (unterstützt `offset`/`limit` für große Dateien) |
| `list_projects` | Alle Projekte im Docs-Repository anzeigen |
| `audit_project` | Cross-Repo-Audit — scannt Docs-Repository und lokales Projekt, schlägt Aktionen vor |
| `init_project` | Dokumente für ein neues Projekt scaffolden (interne+externe Dokumente, selektive Dateierstellung) |
| `validate_docs` | Dokumente gegen Team-Policy (`policy.toml`) validieren |
| `rebuild_index` | Volltextsuchindex neu aufbauen (normalerweise automatisch) |
| `check_doc_changes` | Seit dem letzten Index-Build hinzugefügte, geänderte oder gelöschte Dokumente erkennen |

## CLI

```
alcove              MCP-Server starten (Agenten rufen das auf)
alcove setup        Interaktives Setup — jederzeit erneut ausführen
alcove doctor       Gesundheit der Alcove-Installation prüfen
alcove validate     Dokumente gegen Policy validieren (--format json, --exit-code)
alcove index        Suchindex inkrementell aktualisieren (nur geänderte Dateien)
alcove rebuild      Suchindex von Grund auf neu aufbauen (nach Schema-Änderungen)
alcove search       Dokumente vom Terminal aus suchen
alcove uninstall    Skills, Konfiguration und Legacy-Dateien entfernen
```

## Suche

Alcove wählt automatisch die beste Suchstrategie. Wenn der Suchindex existiert, verwendet es **BM25-Ranking-Suche** (basierend auf [tantivy](https://github.com/quickwit-oss/tantivy)) für relevanzbasierte Ergebnisse. Ohne Index fällt es auf grep zurück. Du musst nie darüber nachdenken.

```bash
# Aktuelles Projekt durchsuchen (automatisch vom CWD erkannt)
alcove search "authentication flow"

# Alle Projekte durchsuchen — deine persönliche Wissensdatenbank
alcove search "OAuth token refresh" --scope global

# Grep-Modus erzwingen für exakte Teilstring-Suche
alcove search "FR-023" --mode grep
```

Der Index wird automatisch im Hintergrund erstellt, wenn der MCP-Server startet, und wird bei Dateiänderungen automatisch neu aufgebaut. Keine Cron-Jobs, keine manuellen Schritte.

**Wie es für Agenten funktioniert:** Agenten rufen einfach `search_project_docs` mit einer Abfrage auf. Alcove kümmert sich um den Rest — Ranking, Deduplizierung (ein Ergebnis pro Datei), projektübergreifende Suche und Fallback. Der Agent muss nie einen Suchmodus wählen.

## Projekterkennung

Standardmäßig erkennt Alcove das aktuelle Projekt aus dem Arbeitsverzeichnis deines Terminals (CWD). Du kannst dies mit der Umgebungsvariable `MCP_PROJECT_NAME` überschreiben:

```bash
MCP_PROJECT_NAME=my-api alcove
```

Nützlich, wenn dein CWD nicht mit einem Projektnamen in deinem Docs-Repository übereinstimmt.

## Dokumentrichtlinie

Definiere teamweite Dokumentationsstandards mit `policy.toml` in deinem Docs-Repository:

```toml
[policy]
enforce = "strict"    # strict | warn

[[policy.required]]
name = "PRD.md"
aliases = ["prd.md", "product-requirements.md"]

[[policy.required]]
name = "ARCHITECTURE.md"

  [[policy.required.sections]]
  heading = "## Overview"
  required = true

  [[policy.required.sections]]
  heading = "## Components"
  required = true
  min_items = 2
```

Policy-Dateien werden mit Priorität aufgelöst: **Projekt** (`<project>/.alcove/policy.toml`) > **Team** (`DOCS_ROOT/.alcove/policy.toml`) > **eingebauter Standard** (core-Dateiliste aus config.toml). Dies stellt konsistente Dokumentqualität über alle Projekte sicher und erlaubt projektspezifische Überschreibungen.

## Konfiguration

Die Konfiguration liegt unter `~/.config/alcove/config.toml`:

```toml
docs_root = "/Users/you/documents"

[core]
files = ["PRD.md", "ARCHITECTURE.md", "PROGRESS.md", "DECISIONS.md", "CONVENTIONS.md", "SECRETS_MAP.md", "DEBT.md"]

[team]
files = ["ENV_SETUP.md", "ONBOARDING.md", "DEPLOYMENT.md", "TESTING.md", ...]

[public]
files = ["README.md", "CHANGELOG.md", "CONTRIBUTING.md", "SECURITY.md", ...]

[diagram]
format = "mermaid"
```

Alles wird interaktiv über `alcove setup` eingestellt. Du kannst die Datei auch direkt bearbeiten.

## Unterstützte Agenten

| Agent | MCP | Skill |
|-------|-----|-------|
| Claude Code | `~/.claude.json` | `~/.claude/skills/alcove/` |
| Cursor | `~/.cursor/mcp.json` | `~/.cursor/skills/alcove/` |
| Claude Desktop | Plattformkonfiguration | — |
| Cline (VS Code) | VS Code globalStorage | `~/.cline/skills/alcove/` |
| OpenCode | `~/.config/opencode/opencode.json` | `~/.opencode/skills/alcove/` |
| Codex CLI | `~/.codex/config.toml` | `~/.codex/skills/alcove/` |
| Copilot CLI | `~/.copilot/mcp-config.json` | `~/.copilot/skills/alcove/` |
| Antigravity | `~/.gemini/antigravity/mcp_config.json` | — |
| Gemini CLI | `~/.gemini/settings.json` | `~/.gemini/skills/alcove/` |

## Unterstützte Sprachen

Das CLI erkennt automatisch deine Systemsprache. Du kannst sie auch mit der Umgebungsvariable `ALCOVE_LANG` überschreiben.

| Sprache | Code |
|---------|------|
| English | `en` |
| 한국어 | `ko` |
| 简体中文 | `zh-CN` |
| 日本語 | `ja` |
| Español | `es` |
| हिन्दी | `hi` |
| Português (Brasil) | `pt-BR` |
| Deutsch | `de` |
| Français | `fr` |
| Русский | `ru` |

```bash
# Sprache überschreiben
ALCOVE_LANG=de alcove setup
```

## Aktualisieren

```bash
# Homebrew
brew upgrade epicsagas/alcove/alcove

# cargo-binstall
cargo binstall alcove

# Aus Quellcode
cargo install alcove
```

## Deinstallieren

```bash
alcove uninstall          # Skills & Konfiguration entfernen
cargo uninstall alcove    # Binary entfernen
```

## Mitwirken

Fehlerberichte, Funktionsanfragen und Pull Requests sind willkommen. Bitte öffne ein Issue auf [GitHub](https://github.com/epicsagas/alcove/issues), um eine Diskussion zu beginnen.

## Lizenz

Apache-2.0
