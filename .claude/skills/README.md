# Project skills

Agent skills that load for anyone working on this repository in Claude Code.

## Vendored from mattpocock/skills

Copied unchanged from [mattpocock/skills](https://github.com/mattpocock/skills) at commit `5beb35b`, under the MIT license in [LICENSE-mattpocock-skills](LICENSE-mattpocock-skills). Only the `agents/openai.yaml` files were dropped.

| Skill | Use it for |
|---|---|
| `diagnosing-bugs` | A hard bug or a slowdown: build a loop that goes red first, then hypothesise, instrument, fix, and leave a regression test. |
| `domain-modeling` | Pinning down terms (revision, run, finding, outbox) in a `GLOSSARY.md`, and recording hard-to-reverse decisions as ADRs in `docs/adr/`. |
| `codebase-design` | Designing or reshaping a module's interface: deep modules, design it twice. |
| `improve-codebase-architecture` | A scan for deepening opportunities, reported as an HTML page, then a grilling on the one you pick. |
| `grilling` | Stress-testing a plan in rounds of numbered questions, each with a recommended answer. `improve-codebase-architecture` calls it. |
| `writing-for-agents` | Writing or editing a skill, `AGENTS.md` or `CLAUDE.md`. |

To update one, copy the folder again from a newer commit and change the commit above.

## pstack

[pstack](https://github.com/michael-denyer/pstack-claude) (MIT) is enabled as a plugin in `.claude/settings.json`, so Claude Code offers to install it on first use. It brings `poteto-mode`, `tdd`, `architect`, `blast-radius`, `thermos` and the rest; it is not copied here.
