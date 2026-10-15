# Agent panel: Claude and Codex in a sheaf window

Status: **proposed** (2026-10-14). Listed under Pending in
[`ROADMAP.md`](../ROADMAP.md#agent-panel-claude-and-codex); expected to
become its own milestone, one of the last. Nothing is built. This memo
records the decisions so far, the research behind them, and a first cut
of the slices.

## The idea

A chat panel in the window where the user works with an AI agent on the
open document, the way Zed runs external agents in its agent threads. The
agent can:
- see the document: pages, text (OCR layers included), bookmarks, marks,
  the selection, and page images;
- see the edit history tree;
- list every command, built-in or from a plugin, with its help text;
- run commands and edits live, with the user's approval;
- analyze the document's state and history.

Foxit's AI Assistant does some of this (agentic editing through chat,
"Delete Page 1"), but only inside Foxit's subscription. See
[prior-art/foxit.md §3.13](../docs/prior-art/foxit.md#313-ai-assistant-ch-15-subscription-only).

## Decisions (Greg, 2026-10-14)

- **Agents only.** sheaf launches an agent program, and the agent handles
  sign-in and billing with the user's own subscription. sheaf takes no
  API keys and has no agent loop of its own.
- **Claude and Codex first.** Other ACP agents (Gemini CLI and the rest of
  the ACP Registry) can come later.
- **Commands run live.** Each approved tool call runs at once. The agent
  doesn't hand back a plan for the user to run.
- **No local models** for now. That's probably its own feature later.
- **Core, not a plugin.** `plugin-queue.md` skipped "Ask Claude" as a
  plugin on 2026-10-13 because AI should be core. Plugins also can't
  start processes or stream UI.
- **UI:** no gpui-kit dependency, per
  [`gpui-kit-eval.md`](../docs/gpui-kit-eval.md); a few small pieces are
  worth copying (see [gpui-kit](#gpui-kit) below). Markdown is our own.

## Signing in

Researched 2026-10-14. This constraint shapes the whole design.

**Claude.** As of 2026-10-14, the Claude Code
[Legal and compliance](https://code.claude.com/docs/en/legal-and-compliance)
page says:
- Third-party developers may not offer Claude.ai login in their own
  apps, or route requests through Free, Pro or Max plan credentials on
  their users' behalf.
- They may not collect, store or intermediate Claude.ai credentials or
  session tokens; sign-in must complete through Anthropic's own flow.
- Developers building on the Agent SDK should use API keys.
- It does allow "an end user signing in to the unmodified Claude Code
  binary with their own Claude subscription".

So sheaf never sees a credential: the user signs in inside the agent.

**Go/no-go for Claude.** Zed runs Claude through `claude-agent-acp`, an
adapter built on the Agent SDK. The terms don't say whether a
subscription sign-in through that adapter counts as the unmodified Claude
Code binary. Since sheaf takes no API keys, the answer decides whether
Claude can use the ACP path. **Ask Anthropic before building it.**

If the answer is no, drive the user's installed `claude` directly in its
headless mode, which is the unmodified binary:
- `claude -p --input-format stream-json --output-format stream-json`;
- sheaf's tools through `--mcp-config`;
- approval prompts back to sheaf through `--permission-prompt-tool`.

That means a second transport beside ACP, just for Claude.

**Codex.** Codex handles its own ChatGPT sign-in, and sheaf runs
`codex-acp`. OpenAI opened "Sign in with ChatGPT" to approved third-party
partners in October 2026, but that's for apps that call OpenAI
themselves, so it doesn't apply to an agents-only design.

## Architecture

```
sheaf chat panel ──ACP (stdio)──▶ claude-agent-acp / codex-acp ──▶ vendor
        ▲                                   │
        └────────── MCP: sheaf's tools ◀────┘
```

- **New crate `sheaf-agent`,** with no GPUI, like `sheaf-plugin`:
  - the ACP client over the agent's stdin and stdout (crate
    `agent-client-protocol`, Apache-2.0, 3.3.0 on 2026-10-14);
  - starting and stopping agents, sessions, streamed updates and
    permission requests;
  - the Claude headless transport, if the go/no-go goes that way.
- **sheaf's tools are an MCP server** (`rmcp`, Apache-2.0). There are two
  ways for the agent to reach it:
  - MCP over the ACP connection itself (`agent-client-protocol-rmcp`,
    behind its `unstable_mcp_over_acp` feature);
  - **`sheaf mcp`**, a stdio MCP server that connects to the running
    sheaf through the single-instance socket (`sheaf-instance`, M9 slice
    1). The socket protocol carries only "open these paths" today, so it
    needs new requests, replies that stream, and a `PROTOCOL` bump.
- **`sheaf mcp` is useful on its own.** Claude Code, Claude Desktop or
  Codex run from outside sheaf could use it with no panel and no sign-in
  questions, so it's the first slice.
- **Which agents:** Claude and Codex built in. A list in `settings.toml`
  (command, arguments, environment) adds or overrides them. The ACP
  Registry comes later.
- **The agent's own tools.** Claude Code and Codex come with shell and
  file-editing tools.
  - sheaf doesn't advertise ACP's `fs` and `terminal` client
    capabilities.
  - It limits each agent to sheaf's tools, through Claude Code's allowed
    and disallowed tools and Codex's sandbox and approval settings.
  - Not checked yet: how much of this each adapter lets a client set. If
    an agent can't be limited, its own tools need the same approval as
    sheaf's writes.

## Tools

Most already exist as the plugin host API (`wit/sheaf.wit`). The MCP
tools translate them.

| Tool | Comes from |
|---|---|
| Document: path, pages, outline, marks per page, selection, current page | WIT `document` |
| Text: page text, search, pattern search, words, text boxes | WIT `text` (unsaved OCR layers included) |
| Page images, as PNG for models that can see | WIT `render`, with caps on size and count |
| History: versions with id, parent, label, author, time and edits in words; the branches | `VersionView`, `describe.rs`, `Document::edge` (M9 slice 4, done) |
| Commands: id, label, category, help, keywords, whether it can run now | `CommandRegistry`. Built-ins can't register without help (`command_info.rs`), nor can plugin commands (manifest loader). |
| Submit edits | `sheaf_doc::Edit` (serde) through `apply_batch`, one version. The same as the plugin `edits` interface. |
| Run a command | The registry's target: a built-in action or `plugin::Run` |
| Go to a version | `Document::jump` |

**Versions keep reads and writes honest.** Every tool returns the current
`VersionId`. Writes take the version they were based on and are refused
when the document has moved since, because the user edited in the
meantime. The agent then reads again.

## Running commands live

**The gap.**
- Built-in commands are mostly argument-free `actions!` that act on the
  selection or open a dialog (OCR, Bates, headers and footers, split…).
  An agent can't fill in a GPUI dialog.
- Plugin commands take `args-json` with no schema, and `show-form` waits
  for the user.

**The plan.**
- **With an argument schema,** a command runs without its dialog. The
  schema comes from schemars, as `plugin::Run`'s already does.
- **Without a schema,** the call opens the command's dialog, pre-filled
  from the arguments where the dialog supports it. It returns when the
  user closes the dialog, as done or cancelled.
  - The user is watching anyway, so this is acceptable.
  - Every command is usable from the first day.
- **Schemas are added over time.** M15 batch actions need the same
  schemas, so the two share that work. With the dialog fallback, this
  milestone doesn't have to wait for M15.
- **Commands that act on the selection** either take the pages in their
  schema, or the agent sets the selection first.
- **Plugins:** an optional `args` schema per command in the manifest.
  `show-form` is answered from the arguments when they cover every field.

**History.**
- Each approved call becomes one version, labeled "<agent>: <command or
  edits in words>".
- Its author is the agent on the user's behalf ("Claude for GE").
  `apply_batch` needs an author parameter for this; today every version
  takes the identity from `settings.toml`.
- **Revert this turn:** a button on each agent turn that jumps to the
  version before the turn's first edit. A jump isn't an edit, so it costs
  nothing and can be redone.

## Permissions

- **Reading the open document** is allowed after the user consents, the
  first time that document goes to a cloud agent.
  - Tax return information falls under IRC §7216, which limits disclosing
    it to third parties without the taxpayer's consent. Firms will ask
    about this first.
  - Open: the wording, and whether consent is per document, per client or
    per user.
- **`settings.toml`:** `[agent] enabled` and the allowed agents. A firm
  can ship settings with agents off. sheaf only reads that file.
- **Every write asks first:** Allow once, Always for this command in this
  thread, or Reject.
  - For sheaf's own tools, sheaf prompts in its MCP handler, so the rule
    holds whatever the agent does.
  - Tools the agent itself asks about arrive as ACP
    `session/request_permission`.
- **Never "always":** Save, Save As, exports and anything else that writes
  files; commands that touch other files (Combine, Split); opening URLs.
- **Plugin commands** run with the plugin's own permissions. The agent
  can't grant more.
- **A log of what was sent:** tool calls and their results, per thread,
  in the local state folder. Open: whether a firm needs to keep it.

## Panel

- A new `Panel::Agent` in the right dock, beside History.
- **Threads:** one per document (one document per window today), kept in
  the state folder, with a list of past threads.
- **Agent picker:** Claude or Codex, with each one's sign-in status.
  "Sign in" runs the agent's own login (ACP `authenticate`, or the agent's
  own login command).
- **Message list:** GPUI's `ListState` with `FollowMode::Tail`, which our
  pinned Zed revision already has (`gpui/src/elements/list.rs:113`); Zed's
  own agent panel uses the same thing. Plus a jump-to-latest button.
- **Replies:** our own Markdown renderer on `pulldown-cmark` (MIT):
  - paragraphs, emphasis, inline code, code blocks without highlighting,
    lists, tables and links;
  - page references ("p. 12") as links that go to the page;
  - while streaming, only the last block is parsed again;
  - a Copy button per reply. Drag-selection across replies can come
    later.
- **Tool-call cards:**
  - collapsible, showing the arguments, and the result in words
    (`describe.rs` for edits);
  - the approval buttons;
  - status: running, done, failed or rejected.
- **ACP `plan` updates** as a checklist.
- **The message box** is `text_area.rs`.
  - Enter sends, Shift+Enter starts a new line. Check against the note
    editor, where Ctrl+Enter confirms.
  - @-mentions of pages, bookmarks and marks use the command palette's
    list.
  - Chips attach the selection or a region (`region.rs`).
  - This is where porting gpui-kit's undo/redo and word movement into
    `text_area.rs` pays off most (the evaluation's first suggestion), if
    that isn't done by then.
- **Cancel** (`session/cancel`), and a working indicator.

## gpui-kit

Checked 2026-10-14 against gpui-kit 0.7.3.

**Licenses:**
- gpui-kit, gpui-component and gpui-base are Apache-2.0, so we can copy
  from them with attribution.
- Zed's `agent_ui`, `acp_thread`, `agent_servers` and `markdown` are
  GPL-3.0-or-later (checked in our Zed checkout). We can study how they
  behave but can't copy their code.

**Depending on it** is still the wrong trade. Even `gpui-base` alone
pulls in `lsp-types`, `html5ever`, `markdown` and `ropey`, and pins
`gpui-pre =0.3.9`.

| Piece | Size | Verdict |
|---|---|---|
| `message_scroller.rs` | 508 lines | Not needed. It wraps GPUI's `ListState` tail-follow mode, which we already have; it only adds a jump-to-latest button. |
| Markdown view, `gpui-base/src/text/` | ~22k lines, plus 4.7k for window-wide selection | The one real candidate: streaming append (`TextViewState::push_str`), re-parsing only what's new, fade-in (`stream_fade.rs`), tables, selection and copy. But it's tightly tied to kit's theme, animation and selection code. Use it as the reference for ours, and copy it only if drag-selection across replies is wanted. |
| `collapsible.rs` | 145 lines | Small; copy or rewrite for tool-call cards. |
| `clipboard.rs` | 163 lines | Small; copy or rewrite for Copy buttons. |
| `shimmer.rs` | 657 lines | Optional polish for "Running OCR…"; a spinner does the job. |
| `message.rs`, `bubble.rs` | ~1k lines | No. Layout slots with little logic, and the agent's output is full-width, not bubbles. |
| `diff/` | 8k lines | No. It shows text patches; sheaf shows an agent's changes as edits in words. |
| Attachments, speech, syntax highlighter | | No. |

## Slices (first cut)

1. **`sheaf mcp`:** read tools, commands and history, through the instance
   socket. Test it with Claude Code and Codex run from a terminal.
2. **Writes:** edits and commands with version checks, the approval prompt
   in sheaf, versions authored by the agent, the dialog fallback.
3. **`sheaf-agent`:** the ACP client. Codex first, since it doesn't wait
   on the go/no-go; then Claude through ACP or through the headless
   fallback.
4. **The panel:** threads, streaming Markdown, tool-call cards, approvals,
   cancel, the message box.
5. **Review:** Revert this turn, page links, @-mentions and attachments.
6. **Policy:** consent, the settings, the send log, user docs.

About the size of M6 (plugins). The protocol crates make the plumbing
small; the panel, the approval and review UX, and argument schemas for
about 90 built-in commands are the bulk.

## Open

- Anthropic's answer on subscription sign-in through `claude-agent-acp`.
- How far each adapter lets sheaf limit the agent to sheaf's tools.
- MCP over ACP (unstable) or the `sheaf mcp` shim inside the panel.
- Threads: per document or per window, how long they're kept.
- §7216 consent: wording, scope, and whether the send log is kept.
- Page images: caps on size and count.
- Commands an agent should never run.
- Placement: after M15, or before it now that the dialog fallback removes
  the dependency.

## Sources

Checked 2026-10-14.
- [Claude Code: Legal and compliance](https://code.claude.com/docs/en/legal-and-compliance)
  (authentication and credential use)
- [Zed docs: External Agents](https://zed.dev/docs/ai/external-agents)
- [Zed blog: The ACP Registry is Live](https://zed.dev/blog/acp-registry)
- [ACP protocol overview](https://agentclientprotocol.com/protocol/overview)
- [`agent-client-protocol`](https://docs.rs/agent-client-protocol/) and
  [`agent-client-protocol-rmcp`](https://docs.rs/agent-client-protocol-rmcp/)
- [Which coding agents support ACP](https://agentrq.com/blog/which-coding-agents-support-acp-agent-client-protocol)
  (the `claude-agent-acp` and `codex-acp` adapters)
- [The New Stack: Sign in with ChatGPT](https://thenewstack.io/sign-in-with-chatgpt/)
  and [OpenAI help](https://help.openai.com/en/articles/20001542-using-your-chatgpt-plan-in-other-apps-and-sites)
- [The Register: Anthropic clarifies ban on third-party tool access](https://www.theregister.com/software/2026/02/20/anthropic-clarifies-ban-on-third-party-tool-access-to-claude/5014546)
