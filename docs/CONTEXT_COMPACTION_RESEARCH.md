# LLM Agent Context Compaction/Compression Strategies Research

> Research date: 2026-03-22

## Table of Contents

1. [Executive Summary](#executive-summary)
2. [Claude Code (Anthropic)](#1-claude-code-anthropic)
3. [Claude API Server-Side Compaction](#2-claude-api-server-side-compaction)
4. [Cursor](#3-cursor)
5. [Aider](#4-aider)
6. [Continue.dev](#5-continuedev)
7. [OpenAI Agents SDK](#6-openai-agents-sdk)
8. [LangChain / LangGraph](#7-langchain--langgraph)
9. [Microsoft Agent Framework (Semantic Kernel)](#8-microsoft-agent-framework-semantic-kernel)
10. [Google ADK](#9-google-adk)
11. [OpenCode (SST)](#10-opencode-sst)
12. [JetBrains Research Findings](#11-jetbrains-research-findings)
13. [Factory.ai Evaluation Results](#12-factoryai-evaluation-results)
14. [Cross-Cutting Analysis](#13-cross-cutting-analysis)
15. [Design Recommendations](#14-design-recommendations)
16. [Sources](#sources)

---

## Executive Summary

Context compaction is the process of reducing conversation history size to stay within LLM context window limits while preserving essential information. Every major coding agent and LLM framework has adopted some form of this, but strategies vary significantly in sophistication, trigger mechanisms, and information preservation quality.

**Key findings:**
- **Observation masking** (replacing old tool outputs with placeholders) often outperforms LLM summarization while being cheaper and simpler (JetBrains research, 500 SWE-bench instances)
- **Structured summarization** with explicit sections (files modified, decisions, errors) outperforms free-form summaries (Factory.ai evaluation, 36,611 messages)
- **Artifact tracking remains unsolved** -- even the best approaches score only 2.45/5.0 on file modification tracking
- **Multi-pass pipelines** (gentle -> moderate -> aggressive) are emerging as the dominant pattern (Microsoft Agent Framework, OpenCode)
- **80% context usage** is the most common compaction trigger threshold across implementations
- **Tool results** are the dominant source of context bloat in coding agents

---

## 1. Claude Code (Anthropic)

### Trigger Mechanism
- **Auto-compact threshold**: ~95-98% of effective context window (total context minus reserved output tokens)
- **Effective window calculation**: For Claude Sonnet with 128k max output tokens, the effective input window is significantly reduced
- **Recommended manual trigger**: ~60% utilization (every 30-45 min of active work or after major milestones)

### Summarization Strategy
- Uses the Claude API's server-side compaction (see section 2)
- Before sending to compaction API: strips images, PDFs, phantom blocks, beta fields
- Single-pass LLM summarization of entire conversation history
- Manual `/compact` command available for proactive compaction

### What's Preserved
- Session names and custom titles
- Plan mode state (planning vs implementation)
- Subagent message history (trimmed to prevent failures)
- Configuration and active state

### What's Discarded/Compressed
- Full tool output details (large outputs saved to disk with file references)
- Image and PDF content blocks
- Progress messages (filtered to prevent memory accumulation)

### Context Component Budget Limits

| Component | Budget |
|-----------|--------|
| MCP tool descriptions | Auto-deferred when >10% of context; discovery via MCPSearch tool |
| Skill descriptions | Limited to 2% of total context window |
| PDFs (>10 pages) | Returned as lightweight references |
| Large tool outputs | Saved to disk with file references |

### Key Implementation Details
- Context buffer reduced to ~33,000 tokens (16.5%) as of early 2026
- 1M context window generally available for Opus 4.6 / Sonnet 4.6 (no pricing premium)
- `clear_tool_uses_20250919` strategy clears old tool results when context grows beyond threshold
- Version 2.1.77 fixed memory growth from progress messages surviving compaction

---

## 2. Claude API Server-Side Compaction

### API Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `type` | string | Required | `"compact_20260112"` |
| `trigger` | object | 150,000 tokens | When to trigger. Minimum: 50,000 tokens |
| `pause_after_compaction` | boolean | `false` | Pause after generating summary |
| `instructions` | string | `null` | Custom summarization prompt (replaces default) |

### How It Works
1. Detects when input tokens exceed trigger threshold
2. Generates a structured summary of conversation
3. Creates a `compaction` content block containing the summary
4. Continues response with compacted context
5. On subsequent requests, API auto-drops all blocks prior to the `compaction` block

### Threshold Guidelines

| Range | Use Case |
|-------|----------|
| Low (5k-20k) | Sequential entity processing; frequent compaction |
| Medium (50k-100k) | Multi-phase workflows |
| High (100k-150k) | Tasks needing substantial context |
| Default (100k) | General long-running tasks |

### Performance Example (5 tickets)
- **Without compaction**: 204,416 input tokens at turn 37
- **With compaction**: 82,171 input tokens at turn 26 (2 compactions) -- **58.6% reduction**

### Custom Summary Prompt
The `instructions` parameter completely replaces the default. Recommended to include sections for:
- Completed work with identifiers
- Progress status
- Next steps

### Limitations
- Does not work optimally with server-side sampling loops (web search, Extended Thinking)
- Inherent information loss
- Not suitable for tasks requiring full audit trails

---

## 3. Cursor

### Context Management Approach
Cursor does not implement traditional conversation compaction. Instead, it uses a **selective context injection** model:

- **Observation masking**: Replaces old environment observations with placeholders while preserving full action and reasoning history
- **File reading limits**: Defaults to first 250 lines per file, extends by 250 if needed
- **Semantic retrieval**: Automatically pulls relevant codebase parts based on estimated relevance (current file, similar patterns, session info)
- **@ symbol directives**: User-directed surgical context injection

### Why Observation Masking Works for Cursor
- Typical SE agent turns heavily skew toward observation (tool output)
- Keeping action + reasoning history while masking observations provides the best cost/quality tradeoff
- Agent retains past reasoning and decisions without reprocessing verbose old text

### No Explicit Compaction Trigger
Cursor manages context proactively through bounded context injection rather than reactive compaction. The model never receives unbounded growing history.

---

## 4. Aider

### Context Architecture
Aider splits context into distinct budgeted sections:

- **Repository map**: Tree-sitter based code structure (default 1k tokens, expandable)
- **Chat files**: Explicitly added files for editing
- **Chat history**: Conversation with summarization

### Repository Map Strategy
- Uses tree-sitter to extract code definitions and references
- Builds NetworkX MultiDiGraph of file dependencies
- Ranks nodes using **PageRank with personalization** to find most relevant files
- Formats top-ranked definitions into token-limited context string
- When no chat files: budget expands to `min(map_tokens * 2, max_context - 4096)`

### Chat History Compaction
- **Message state**: `cur_messages` (current segment) and `done_messages` (completed segments)
- **Trigger**: When `done_messages` exceeds size limit
- **Summarization**: Background thread calls `ChatSummary.summarize()` using a **weak/cheap model**
- **Strategy**: Replaces detailed history with high-level summary while preserving conversation continuity

### Manual Controls
- `/tokens` -- view current token usage
- `/drop` -- remove files from context
- `/clear` -- clear chat history

---

## 5. Continue.dev

### Context Strategy
Continue.dev does **not implement automatic context compaction**. Instead, it relies on:

- **Selective context providers**: code, docs, diff, terminal, problems, folder, codebase
- **Agent-driven file exploration**: Built-in tools for search and file reading replace static context injection
- **Rules files**: `.continue/rules` provides project-specific context
- **MCP integration**: Dynamic context through Model Context Protocol
- **Local-first architecture**: Only necessary context sent to cloud LLMs

### No Compaction
Continue delegates context management to the model's own reasoning through tool use, rather than implementing explicit compaction. This is a "let the agent manage its own context" approach.

---

## 6. OpenAI Agents SDK

### Built-in Truncation

| Mode | Behavior |
|------|----------|
| `"auto"` | Silently removes oldest messages until prompt fits context window |
| `"disabled"` | Throws `context_length_exceeded` error if limit exceeded |
| `None` (default) | No truncation |

### Session Memory Management

#### TrimmingSession (Last-N Turns)
- Keeps only the N most recent user turns
- Scans backward to find Nth user message, discards everything before
- Preserves complete turn structures (user + all subsequent items)
- **No LLM call needed** -- deterministic, reproducible
- Best for: independent sequential tasks, recent-context-dominant conversations

#### SummarizingSession
- Two-phase memory: recent turns verbatim + summarized older history
- Triggers when total exceeds `context_limit`
- Summarizes everything before `keep_last_n_turns`
- Injects synthetic user/assistant pair with summary
- Maintains metadata distinguishing real from synthetic messages

### Summarization Prompt Design (Recommended)
- Contradiction checking against system instructions
- Temporal ordering with timestamps
- Hallucination controls (marking uncertain facts as "UNVERIFIED")
- Structured sections: Product/Environment, Issues, Steps Tried, Identifiers, Timeline, Tool Performance, Status/Blockers, Next Steps

### Evaluation Approach
- Compare baseline vs modified conversation accuracy
- LLM-as-judge for summary quality grading
- Replay transcripts to measure next-turn accuracy
- Track error regression patterns
- Monitor token pressure for critical context loss detection

---

## 7. LangChain / LangGraph

### ConversationSummaryBufferMemory
- Hybrid of `ConversationSummaryMemory` + `ConversationBufferWindowMemory`
- **Summarizes** earliest interactions while keeping **most recent N tokens** verbatim
- Configurable `max_token_limit` threshold
- **LLM-generated summaries** (uses the model itself to compress)
- Prunes automatically when token limit exceeded

### Autonomous Context Compression (Deep Agents SDK)
- Agent-initiated compression via tool middleware
- Retains most recent **10% of available context** verbatim, summarizes the rest
- **Intelligent trigger timing** -- agent decides when to compress:
  - Task transitions where prior context becomes irrelevant
  - After extracting key results from extensive context
  - Before consuming large new information blocks
  - Before complex multi-step processes
  - When decisions invalidate previous context
- Integration: `create_summarization_tool_middleware` in agent middleware list
- CLI: `/compact` command for manual triggering
- Philosophy: "harnesses should get out of the way" -- leverage model reasoning for timing

### Key Limitation
Information loss is inherent. Summarization compresses key details which "may occasionally be lost." No structured preservation of file paths, error codes, or specific technical details unless custom prompts are provided.

---

## 8. Microsoft Agent Framework (Semantic Kernel)

### Architecture: The Most Sophisticated Pipeline System

Microsoft provides the most comprehensive compaction framework with composable strategies, explicit trigger/target separation, and atomic message group handling.

### Message Groups (Atomic Units)

| Kind | Description |
|------|-------------|
| `System` | Always preserved during compaction |
| `User` | Single user message starting a turn |
| `AssistantText` | Plain text response (no tool calls) |
| `ToolCall` | Assistant tool call + results (atomic -- never split) |
| `Summary` | Condensed message from summarization |

### Trigger System

| Trigger | Fires When |
|---------|-----------|
| `TokensExceed(n)` | Token count exceeds threshold |
| `MessagesExceed(n)` | Message count exceeds threshold |
| `TurnsExceed(n)` | User turn count exceeds threshold |
| `GroupsExceed(n)` | Group count exceeds threshold |
| `HasToolCalls()` | Non-excluded tool call groups exist |
| `Always` / `Never` | Unconditional / disabled |

Triggers are combinable with `All(...)` (AND) and `Any(...)` (OR).

### Trigger vs Target (Dual Predicate)
- **Trigger**: Controls _when_ compaction begins
- **Target**: Controls _when_ compaction stops
- Default target: inverse of trigger condition

### Strategy Pipeline (Gentle -> Aggressive)

| # | Strategy | Aggressiveness | Preserves Context | Requires LLM |
|---|----------|---------------|-------------------|---------------|
| 1 | `ToolResultCompaction` | Low | High (only collapses tool results) | No |
| 2 | `SummarizationCompaction` | Medium | Medium (replaces history with summary) | Yes |
| 3 | `SlidingWindowCompaction` | High | Low (drops entire turns) | No |
| 4 | `TruncationCompaction` | High | Low (drops oldest groups) | No |

### Python: TokenBudgetComposedStrategy
- Drives pipeline with a token budget target
- Each child strategy runs in order
- `early_stop=True` stops when budget is satisfied
- Built-in fallback excludes oldest groups if strategies alone cannot reach target

### Best Practices
- Use smaller/cheaper model for summarization (e.g., gpt-4o-mini)
- Place gentlest strategies first in pipeline
- Tool result compaction as first pass reclaims the most space cheaply
- Before/after strategies: `before_strategy` compacts before model call, `after_strategy` compacts persisted history

---

## 9. Google ADK

### Sliding Window Approach
- Compresses agent workflow **event history** within sessions
- Triggers at configurable event intervals (`compaction_interval`)
- Example: `compaction_interval=3` compresses after events 3, 6, 9, ...

### Configuration

| Parameter | Description |
|-----------|-------------|
| `compaction_interval` | Events between compression cycles |
| `overlap_size` | Events carried forward for continuity |
| `summarizer` | Custom LLM model for summarization |
| `prompt_template` | Custom summarization prompt |

### Continuity Mechanism
- `overlap_size=1`: Last event from each window carries into next compression batch
- Ensures context continuity across compression boundaries
- LLM-based summarization (customizable model and prompt)

---

## 10. OpenCode (SST)

### Token Tracking
Usable context = Total window - Reserved output tokens - Safety buffer (20,000 tokens default via `COMPACTION_BUFFER`)

### Compaction Flow
1. **Detection**: `isOverflow()` checks if tokens exceed usable context after each assistant message
2. **Marking**: Inserts `CompactionPart` to queue summarization
3. **Processing**: LLM generates structured summary with 5 sections:
   - **Goal**: What user is trying to accomplish
   - **Instructions**: Important user specs/requirements
   - **Discoveries**: Notable learnings from conversation
   - **Accomplished**: Completed vs in-progress items
   - **Relevant Files**: Structured list of paths read or edited
4. **Media conversion**: Images/PDFs become text placeholders
5. **Recovery**: Replays last user message or inserts synthetic continuation
6. **Pruning**: Removes old tool outputs post-loop

### Tool Output Pruning (Separate from Summarization)
- Scans backward, skipping last 2 user turns
- Protects at least **40,000 tokens** of recent tool output
- Removes detailed output from old tool calls while keeping evidence they executed
- Protected tools (e.g., "skill") are never pruned

### Failure Mode
If compaction itself exceeds context limits, session enters **unrecoverable state** with `ContextOverflowError`.

---

## 11. JetBrains Research Findings

### Study: 500 instances on SWE-bench Verified (Dec 2025)

**Observation Masking vs LLM Summarization:**

| Metric | Observation Masking | LLM Summarization |
|--------|--------------------|--------------------|
| Cost reduction | >50% | >50% |
| Solve rate (Qwen3-Coder 480B) | +2.6% improvement | Baseline |
| Cost (Qwen3-Coder 480B) | 52% less than summarization | Baseline |
| Trajectory length | Normal | ~13-15% longer |
| Summary cost overhead | 0% | >7% of total instance cost |
| Performance vs unmanaged | Matched or exceeded in 4/5 configs | Matched in 1/5 configs |

### Key Insights
- **Observation masking matched or beat summarization in 4 of 5 test configurations**
- LLM summarization causes **trajectory elongation** -- agents run ~15% longer
- Results depend on agent architecture (OpenHands needs larger windows than SWE-agent)
- Hyperparameter tuning required per agent type -- no universal solution
- **Recommended**: Hybrid approach -- masking as primary, strategic summarization selectively

---

## 12. Factory.ai Evaluation Results

### Study: 36,611 messages across real SE tasks

**Three Approaches Compared:**

| Method | Overall Score | Accuracy | Context Awareness | Artifact Trail |
|--------|--------------|----------|-------------------|----------------|
| Factory (structured) | **3.70** | **4.04** | **4.01** | **2.45** |
| Anthropic (built-in) | 3.44 | 3.43 | 3.56 | 2.19 |
| OpenAI (opaque) | 3.35 | N/A | N/A | N/A |

### Compression Ratios
- **OpenAI `/responses/compact`**: 99.3% compression (opaque, uninterpretable)
- **Anthropic**: 7-12k character summaries, regenerated each cycle
- **Factory**: Anchored iterative summarization (merges into persistent summary)

### Critical Finding
**Artifact tracking (file modification tracking) remains unsolved.** Even Factory's best score is only 2.45/5.0. This suggests file path/diff tracking needs specialized handling beyond general summarization.

### Factory's Approach: Anchored Iterative Summarization
- Maintains **persistent structured summary** with explicit sections:
  - Session intent
  - File modifications
  - Decisions made
  - Next steps
- New information **merges into** existing summary rather than regenerating from scratch
- Prevents the "re-reading loop" (summarization loses details -> agent re-reads -> cycle repeats)

---

## 13. Cross-Cutting Analysis

### What Triggers Compaction?

| System | Trigger | Threshold |
|--------|---------|-----------|
| Claude Code (auto) | % of effective window | ~95-98% |
| Claude Code (recommended manual) | % of effective window | ~60% |
| Claude API | Absolute token count | 150,000 (default), min 50,000 |
| Cursor | N/A | Proactive bounded injection |
| Aider | Message size limit | Configurable |
| OpenAI Agents SDK | Context limit exceeded | Configurable |
| Microsoft Agent Framework | Token/message/turn/group count | Fully configurable |
| Google ADK | Event count interval | Configurable (e.g., every 3 events) |
| OpenCode | % of usable context | ~100% (usable = total - output - 20k buffer) |
| LangChain | Token limit | `max_token_limit` parameter |
| LangGraph (autonomous) | Agent decides | Model reasoning |

### What's Preserved vs Discarded?

| Information Type | Typically Preserved | Typically Discarded |
|-----------------|--------------------|--------------------|
| System prompt | Always | Never |
| Recent N turns | Always (verbatim) | N/A |
| User intent/goal | In summary | Detailed formulation |
| File paths modified | In summary (weak) | File contents |
| Tool call facts | That they occurred | Full output details |
| Error messages | Key errors in summary | Full stack traces |
| Decisions made | In summary | Reasoning process |
| Images/PDFs | Never (stripped) | Always |
| Code blocks (<50 lines) | Sometimes verbatim | Longer blocks |
| Progress messages | Usually discarded | Yes |

### Single-Pass vs Multi-Pass

| Approach | Used By | Characteristics |
|----------|---------|-----------------|
| **Single-pass summarization** | Claude Code, Aider, LangChain, Google ADK | One LLM call, replace history with summary |
| **Multi-pass pipeline** | Microsoft Agent Framework, OpenCode | Gentle->aggressive stages; stop when budget met |
| **Observation masking** | Cursor, JetBrains (recommended) | No LLM call; replace old observations with placeholders |
| **Iterative/anchored** | Factory.ai | Merge new info into persistent summary structure |
| **Autonomous** | LangGraph Deep Agents | Agent decides when/what to compress |

### Adaptive Strategies (Tool-Heavy vs Conversation-Heavy)

Most systems do **not** explicitly adapt strategy based on conversation type. Notable exceptions:
- **Microsoft Agent Framework**: `HasToolCalls()` trigger allows tool-aware compaction
- **OpenCode**: Separate tool output pruning from conversation summarization
- **Claude Code**: `clear_tool_uses_20250919` specifically targets tool results
- **Factory.ai**: Structured sections inherently adapt (file modifications section only filled when relevant)

### Information Loss Mitigation Techniques

| Technique | Used By | Description |
|-----------|---------|-------------|
| **Structured summary sections** | Factory, OpenCode, OpenAI SDK | Explicit sections for files, decisions, errors |
| **Virtual file preservation** | Lethain pattern | Post-compaction, save original as accessible virtual file |
| **Tool output to disk** | Claude Code | Large outputs saved as file references |
| **Verbatim error preservation** | ForgeCode, OpenCode | Error messages + stack traces kept exactly |
| **Verbatim code blocks** | ForgeCode | Code <50 lines preserved completely |
| **Overlap/overlap_size** | Google ADK | Last N events carry forward across boundaries |
| **Protected recent turns** | OpenCode, Aider, OpenAI SDK | Last 2 turns / last N messages never touched |
| **Protected tool types** | OpenCode | Specific tools (e.g., "skill") never pruned |
| **Anchored iterative merge** | Factory.ai | Merge into persistent summary vs regenerate |
| **Weak model for summarization** | Aider, Microsoft (recommended) | Cheaper model for summaries; save main model budget |

---

## 14. Design Recommendations

Based on this research, here are recommendations for implementing context compaction:

### Architecture
1. **Multi-pass pipeline** (Microsoft pattern): Start with tool result collapsing, then summarization, then truncation as emergency backstop
2. **Separate tool pruning from conversation summarization** (OpenCode pattern): Tool results and conversation context have different information profiles
3. **Structured summary template** (Factory/OpenCode pattern): Force sections for goal, files, decisions, errors, next steps

### Trigger Strategy
1. **Primary trigger**: 80% of usable context window
2. **Emergency trigger**: 95% with more aggressive strategy
3. **Manual trigger**: Always available as `/compact` command
4. **Proactive recommendation**: Suggest compaction at 60% after major milestones

### Preservation Priority (Highest to Lowest)
1. System prompt (never touch)
2. Last 2-3 user turns + responses (verbatim)
3. Active file paths and modification tracking
4. Error messages with stack traces (verbatim)
5. Key decisions and their reasoning
6. Tool call facts (what was called, not full output)
7. Goal/intent statement
8. Older conversation details

### Cost Optimization
1. Use cheaper model for summarization (Claude Haiku, GPT-4o-mini)
2. Prefer observation masking over LLM summarization when possible (52% cheaper per JetBrains)
3. Tool result collapsing is the highest-ROI first step (no LLM call needed)
4. Anchored iterative summaries avoid regeneration cost

### Unsolved Problems
1. **File artifact tracking**: Best systems score only 2.45/5.0 -- consider maintaining a separate artifact registry outside the summary
2. **Re-reading loops**: Summarization can lose details agents need, forcing re-reads. Mitigation: virtual file access to pre-compaction state
3. **Compaction timing**: Too early loses information unnecessarily; too late risks overflow. Autonomous agent-driven timing is promising but under-tested
4. **Evaluation**: No standardized benchmark for compaction quality. Factory.ai's probe-based evaluation is the most rigorous approach found

---

## Sources

### Claude Code / Anthropic
- [Compaction - Claude API Docs](https://platform.claude.com/docs/en/build-with-claude/compaction)
- [Automatic Context Compaction Cookbook](https://platform.claude.com/cookbook/tool-use-automatic-context-compaction)
- [Context Window & Compaction | Claude Code DeepWiki](https://deepwiki.com/anthropics/claude-code/3.3-session-and-conversation-management)
- [Claude Code Context Buffer: The 33K-45K Token Problem](https://claudefa.st/blog/guide/mechanics/context-buffer-management)
- [Claude Code Compaction | Steve Kinney](https://stevekinney.com/courses/ai-development/claude-code-compaction)
- [Session Memory Compaction Cookbook](https://platform.claude.com/cookbook/misc-session-memory-compaction)
- [What is Claude Code Auto Compact | ClaudeLog](https://claudelog.com/faqs/what-is-claude-code-auto-compact/)

### Cursor
- [Mastering Context Management in Cursor | Steve Kinney](https://stevekinney.com/courses/ai-development/cursor-context)
- [Cursor – Working with Context](https://docs.cursor.com/guides/working-with-context)
- [Coding Smarter with LLMs: Leveraging Attention and Context Augmentation in Cursor](https://medium.com/lightricks-tech-blog/coding-smarter-with-llms-leveraging-attention-and-context-augmentation-in-cursor-b004a98a9b3d)

### Aider
- [Token Limits | Aider](https://aider.chat/docs/troubleshooting/token-limits.html)
- [Repository Map | Aider](https://aider.chat/docs/repomap.html)
- [Repository Mapping System | Aider DeepWiki](https://deepwiki.com/Aider-AI/aider/4.1-repository-mapping)
- [Message and Chat Management | Aider DeepWiki](https://deepwiki.com/Aider-AI/aider/3.3-function-based-coders)

### OpenAI
- [Context Engineering - Session Memory | OpenAI Agents SDK](https://developers.openai.com/cookbook/examples/agents_sdk/session_memory)
- [Context Management - OpenAI Agents SDK](https://openai.github.io/openai-agents-python/context/)
- [Truncation in ModelSettings | GitHub Issue #1494](https://github.com/openai/openai-agents-python/issues/1494)

### LangChain / LangGraph
- [ConversationSummaryBufferMemory Docs](https://python.langchain.com/api_reference/langchain/memory/langchain.memory.summary_buffer.ConversationSummaryBufferMemory.html)
- [Conversational Memory in LangChain | Aurelio AI](https://www.aurelio.ai/learn/langchain-conversational-memory)
- [Conversational Memory for LLMs with LangChain | Pinecone](https://www.pinecone.io/learn/series/langchain/langchain-conversational-memory/)
- [Autonomous Context Compression | LangChain Blog](https://blog.langchain.com/autonomous-context-compression/)

### Microsoft
- [Compaction | Microsoft Agent Framework](https://learn.microsoft.com/en-us/agent-framework/agents/conversations/compaction)

### Google
- [Context Compression - Agent Development Kit (ADK)](https://google.github.io/adk-docs/context/compaction/)

### OpenCode
- [Context Management and Compaction | OpenCode DeepWiki](https://deepwiki.com/sst/opencode/2.4-context-management-and-compaction)

### Research & Analysis
- [Cutting Through the Noise: Smarter Context Management for LLM-Powered Agents | JetBrains Research](https://blog.jetbrains.com/research/2025/12/efficient-context-management/)
- [Evaluating Context Compression for AI Agents | Factory.ai](https://factory.ai/news/evaluating-compression)
- [Compressing Context | Factory.ai](https://factory.ai/news/compressing-context)
- [Building an Internal Agent: Context Window Compaction | Lethain](https://lethain.com/agents-context-compaction/)
- [How We Extended LLM Conversations by 10x with Intelligent Context Compaction](https://dev.to/amitksingh1490/how-we-extended-llm-conversations-by-10x-with-intelligent-context-compaction-4h0a)
- [The Fundamentals of Context Management and Compaction in LLMs | Medium](https://kargarisaac.medium.com/the-fundamentals-of-context-management-and-compaction-in-llms-171ea31741a2)
- [Context Window Management Strategies | Maxim.ai](https://www.getmaxim.ai/articles/context-window-management-strategies-for-long-context-ai-agents-and-chatbots/)
- [Two Experiments We Need to Run on AI Agent Compaction | Jason Liu](https://jxnl.co/writing/2025/08/30/context-engineering-compaction/)
- [Compaction vs Summarization: Agent Context Management Compared | Morph](https://www.morphllm.com/compaction-vs-summarization)
