<p align="center">
  <img src="../alcove.png" alt="Alcove" width="100%" />
</p>

<p align="center">Um lugar tranquilo para a documentacao do seu projeto.</p>

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

O Alcove permite que qualquer agente de codificacao com IA leia a documentacao privada do seu projeto — sem vaza-la em repositorios publicos.

Mantenha PRDs, decisoes de arquitetura, mapas de segredos e runbooks internos em um so lugar. Todo agente compativel com MCP recebe o mesmo acesso, em todos os projetos, sem configuracao por projeto.

## O problema

Voce tem documentos internos que nao devem estar no seu repositorio publico do GitHub. Mas seu agente de IA nao consegue te ajudar adequadamente se nao puder le-los — ele inventa requisitos e ignora restricoes que voce ja documentou.

Multiplique isso por varios projetos e varios agentes. Cada um tem configuracao diferente. Toda vez que voce troca, perde o contexto. E nao existe uma forma padrao de organizar ou validar nada disso.

## Como o Alcove resolve isso

O Alcove mantem todos os seus documentos privados em **um unico repositorio compartilhado**, organizado por projeto. Qualquer agente compativel com MCP os acessa da mesma forma — seja no Claude Code, Cursor, Gemini CLI ou Codex.

```
~/projects/my-app $ claude "como a autenticacao e implementada?"

  → Alcove detecta o projeto: my-app
  → Le ~/documents/my-app/ARCHITECTURE.md
  → Agente responde com o contexto real do projeto
```

```
~/projects/my-api $ codex "revise o design da API"

  → Alcove detecta o projeto: my-api
  → Mesma estrutura de documentos, mesmo padrao de acesso
  → Projeto diferente, mesmo fluxo de trabalho
```

**Troque de agente a qualquer momento. Troque de projeto a qualquer momento. A camada de documentos permanece padronizada.**

## O que ele faz

- **Um repositorio de documentos, varios projetos** — documentos privados organizados por projeto, gerenciados em um unico lugar
- **Uma configuracao, qualquer agente** — configure uma vez, todo agente compativel com MCP recebe o mesmo acesso
- **Detecta automaticamente seu projeto** a partir do CWD — sem necessidade de configuracao por projeto
- **Acesso com escopo** — cada projeto ve apenas seus proprios documentos
- **Busca inteligente** — busca BM25 com ranking e indexacao automatica; encontra os documentos mais relevantes primeiro, recorre ao grep quando necessario
- **Busca entre projetos** — busque em todos os projetos de uma vez com `scope: "global"` — use como base de conhecimento pessoal
- **Documentos privados permanecem privados** — documentos sensiveis (mapa de segredos, decisoes internas, divida tecnica) nunca tocam seu repositorio publico
- **Estrutura de documentos padronizada** — `policy.toml` garante documentos consistentes em todos os projetos e equipes
- **Auditoria entre repositorios** — encontra documentos internos mal posicionados no repositorio do projeto, sugere correcoes
- **Validacao de documentos** — verifica arquivos ausentes, templates nao preenchidos, secoes obrigatorias
- **Funciona com mais de 9 agentes** — Claude Code, Cursor, Claude Desktop, Cline, OpenCode, Codex, Copilot, Antigravity, Gemini CLI

## Por que Alcove

| Sem Alcove | Com Alcove |
|------------|------------|
| Documentos internos espalhados entre Notion, Google Docs, arquivos locais | Um repositorio de documentos, estruturado por projeto |
| Cada agente de IA configurado separadamente para acesso a documentos | Uma configuracao, todos os agentes compartilham o mesmo acesso |
| Trocar de projeto significa perder o contexto dos documentos | Deteccao automatica por CWD, troca instantanea de projeto |
| Buscas do agente retornam linhas aleatorias | Busca BM25 com ranking — melhores correspondencias primeiro, indexacao automatica |
| "Buscar todas as minhas notas sobre autenticacao" — impossivel | Busca global em todos os projetos em uma unica consulta |
| Documentos sensiveis com risco de vazar em repositorios publicos | Documentos privados fisicamente separados dos repositorios de projeto |
| Estrutura de documentos varia por projeto e membro da equipe | `policy.toml` garante padroes em todos os projetos |
| Sem como verificar se os documentos estao completos | `validate` detecta arquivos ausentes, templates vazios, secoes faltando |

## Inicio rapido

```bash
cargo install alcove
alcove setup
```

Isso e tudo. `setup` guia voce por tudo interativamente:

1. Onde seus documentos ficam
2. Quais categorias de documentos rastrear
3. Formato de diagrama preferido
4. Quais agentes de IA configurar (MCP + arquivos de habilidades)

Execute `alcove setup` novamente a qualquer momento para alterar as configuracoes. Ele lembra das suas escolhas anteriores.

## Instalar a partir do codigo-fonte

```bash
git clone https://github.com/epicsagas/alcove.git
cd alcove
make install
```

## Como funciona

```mermaid
flowchart LR
    subgraph Projects["Seus projetos"]
        A1["my-app/\n  src/ ..."]
        A2["my-api/\n  src/ ..."]
    end

    subgraph Docs["Seus documentos privados (um repositorio)"]
        D1["my-app/\n  PRD.md\n  ARCH.md"]
        D2["my-api/\n  PRD.md\n  ..."]
        P1["policy.toml"]
    end

    subgraph Agents["Qualquer agente MCP"]
        AG1(Claude Code)
        AG2(Cursor)
        AG3(Gemini CLI)
        AG4(Codex)
        AG5(Copilot)
    end

    subgraph MCP["Servidor MCP Alcove"]
        T1(overview)
        T2("search (BM25 + grep)")
        T3(get_file)
        T4(audit)
        T5(init)
        T6(list)
        T7(validate)
        T8(rebuild_index)
    end

    subgraph Index["Indice de busca"]
        IDX["tantivy BM25\n(auto-construido)"]
    end

    A1 -- "CWD detectado" --> D1
    A2 -- "CWD detectado" --> D2
    Agents -- "stdio MCP" --> MCP
    MCP -- "somente leitura" --> Docs
    MCP -- "busca com ranking" --> Index
    Index -. "construido a partir de" .-> Docs
```

Seus documentos sao organizados em um diretorio separado (`DOCS_ROOT`), uma pasta por projeto. O Alcove le a partir dali e serve para qualquer agente de IA compativel com MCP via stdio. Seu agente chama ferramentas como `get_doc_file("PRD.md")` e obtem respostas especificas do projeto — independentemente de qual agente voce esta usando.

## Classificacao de documentos

O Alcove classifica documentos nos seguintes niveis:

| Classificacao | Onde fica | Exemplos |
|---------------|-----------|----------|
| **doc-repo-required** | Alcove (privado) | PRD, Arquitetura, Decisoes, Convencoes |
| **doc-repo-supplementary** | Alcove (privado) | Implantacao, Integracao, Testes, Runbook |
| **reference** | Alcove pasta `reports/` | Relatorios de auditoria, benchmarks, analises |
| **project-repo** | Seu repositorio GitHub (publico) | README, CHANGELOG, CONTRIBUTING |

A ferramenta `audit` escaneia o repositorio de documentos e o diretorio local do projeto, e sugere acoes — como gerar um README publico a partir do seu PRD privado, ou mover relatorios mal posicionados de volta para o alcove.

## Ferramentas MCP

| Ferramenta | O que faz |
|------------|-----------|
| `get_project_docs_overview` | Lista todos os documentos com classificacao e tamanhos |
| `search_project_docs` | Busca inteligente — seleciona automaticamente BM25 com ranking ou grep, suporta `scope: "global"` para busca entre projetos |
| `get_doc_file` | Le um documento especifico pelo caminho (suporta `offset`/`limit` para arquivos grandes) |
| `list_projects` | Mostra todos os projetos no seu repositorio de documentos |
| `audit_project` | Auditoria entre repositorios — escaneia o repo de documentos e o projeto local, sugere acoes |
| `init_project` | Cria estrutura de documentos para um novo projeto (documentos internos+externos, criacao seletiva) |
| `validate_docs` | Valida documentos contra a politica da equipe (`policy.toml`) |
| `rebuild_index` | Reconstroi o indice de busca de texto completo (normalmente automatico) |

## CLI

```
alcove              Inicia o servidor MCP (agentes chamam isso)
alcove setup        Configuracao interativa — execute novamente a qualquer momento para reconfigurar
alcove validate     Valida documentos contra a politica (--format json, --exit-code)
alcove index        Constroi ou reconstroi o indice de busca
alcove search       Busca documentos pelo terminal
alcove uninstall    Remove habilidades, configuracao e arquivos legados
```

## Busca

O Alcove seleciona automaticamente a melhor estrategia de busca. Quando o indice de busca existe, usa **busca BM25 com ranking** (baseada em [tantivy](https://github.com/quickwit-oss/tantivy)) para resultados ordenados por relevancia. Quando nao existe, recorre ao grep. Voce nunca precisa pensar nisso.

```bash
# Buscar no projeto atual (auto-detectado pelo CWD)
alcove search "authentication flow"

# Buscar em TODOS os projetos — sua base de conhecimento pessoal
alcove search "OAuth token refresh" --scope global

# Forcar modo grep se precisar de correspondencia exata de substrings
alcove search "FR-023" --mode grep
```

O indice e construido automaticamente em segundo plano quando o servidor MCP inicia, e reconstruido quando detecta mudancas nos arquivos. Sem cron jobs, sem etapas manuais.

**Como funciona para agentes:** agentes simplesmente chamam `search_project_docs` com uma consulta. O Alcove cuida do resto — ranking, deduplicacao (um resultado por arquivo), busca entre projetos e fallback. O agente nunca precisa escolher um modo de busca.

## Deteccao de projeto

Por padrao, o Alcove detecta o projeto atual a partir do diretorio de trabalho do seu terminal (CWD). Voce pode sobrescrever com a variavel de ambiente `MCP_PROJECT_NAME`:

```bash
MCP_PROJECT_NAME=my-api alcove
```

Util quando seu CWD nao corresponde a um nome de projeto no seu repositorio de documentos.

## Politica de documentos

Defina padroes de documentacao para toda a equipe com `policy.toml` no seu repositorio de documentos:

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

Arquivos de politica sao resolvidos com prioridade: **projeto** (`<project>/.alcove/policy.toml`) > **equipe** (`DOCS_ROOT/.alcove/policy.toml`) > **padrao integrado** (lista de arquivos core do config.toml). Isso garante qualidade consistente dos documentos em todos os seus projetos, permitindo substituicoes por projeto.

## Configuracao

A configuracao fica em `~/.config/alcove/config.toml`:

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

Tudo isso e configurado interativamente via `alcove setup`. Voce tambem pode editar o arquivo diretamente.

## Agentes suportados

| Agente | MCP | Habilidade |
|--------|-----|------------|
| Claude Code | `~/.claude.json` | `~/.claude/skills/alcove/` |
| Cursor | `~/.cursor/mcp.json` | `~/.cursor/skills/alcove/` |
| Claude Desktop | configuracao da plataforma | — |
| Cline (VS Code) | VS Code globalStorage | `~/.cline/skills/alcove/` |
| OpenCode | `~/.config/opencode/opencode.json` | `~/.opencode/skills/alcove/` |
| Codex CLI | `~/.codex/config.toml` | `~/.codex/skills/alcove/` |
| Copilot CLI | `~/.copilot/mcp-config.json` | `~/.copilot/skills/alcove/` |
| Antigravity | `~/.gemini/antigravity/mcp_config.json` | — |
| Gemini CLI | `~/.gemini/settings.json` | `~/.gemini/skills/alcove/` |

## Idiomas suportados

O CLI detecta automaticamente o locale do seu sistema. Voce tambem pode substitui-lo com a variavel de ambiente `ALCOVE_LANG`.

| Idioma | Codigo |
|--------|--------|
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
# Substituir idioma
ALCOVE_LANG=pt-BR alcove setup
```

## Atualizar

```bash
cargo install alcove
```

## Desinstalar

```bash
alcove uninstall          # remove habilidades e configuracao
cargo uninstall alcove    # remove o binario
```

## Licenca

Apache-2.0
