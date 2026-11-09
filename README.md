# crier

Watches the top of the [Hacker News](https://news.ycombinator.com) front page
and [Lobsters](https://lobste.rs) "hottest", and posts every story that is new
to it into a Telegram channel. Each post is written by an LLM from the linked
article and the top comments of the discussion.

```
<b>systemd adds Rust for leaf binaries but not for shared libraries</b>

systemd merged Rust support that allows writing individual systemd binaries in
Rust while keeping the core shared library C-only. …

💬 Commenters note "loves Rust" really means strict limits: no std, no
external deps, and Rust only in leaf binaries for now.

amutable.com · Lobsters 33▲ 16💬
#rust #systemd
```

## How it works

Every `CRIER_POLL_INTERVAL_SECONDS` (default 10 min):

1. Fetch the top `CRIER_HN_TOP` HN stories and top `CRIER_LOBSTERS_TOP`
   Lobsters stories (10 each by default).
2. Record each one in SQLite. The same link on both sites is one story
   (URLs are normalized: scheme, `www.`, trailing slash, `utm_*` dropped).
3. For each story not seen before: fetch the article text and up to 5
   top-level comments, ask the LLM for a headline, a 2–3 sentence summary, a
   one-line take on the discussion and hashtags, render it as Telegram HTML and
   send it.
4. If an already-posted story later appears on the other site, the existing
   message is edited to link both discussions.

On the very first run (empty database) only `CRIER_INITIAL_POSTS` (3) stories
are posted; the rest of the current front pages are marked as seen.

Failures are retried on the next cycles. After 3 failed LLM attempts the story
is posted with its title and links only; after 6 failed cycles it is given up
(`status = 'failed'` in the database).

## Configuration

| Variable | Default | |
|---|---|---|
| `TELEGRAM_BOT_TOKEN` | – | from @BotFather |
| `TELEGRAM_CHAT_ID` | – | `@channel` or numeric `-100…` id |
| `OPENAI_API_KEY` | – | |
| `CRIER_OPENAI_URL` | OpenAI chat completions | any OpenAI-compatible endpoint with JSON-schema output |
| `CRIER_OPENAI_MODEL` | `gpt-5-mini` | |
| `CRIER_OPENAI_REASONING_EFFORT` | unset | e.g. `low` for OpenAI reasoning models |
| `CRIER_POST_LANGUAGE` | `English` | |
| `CRIER_DATABASE_URL` | `sqlite://crier.sqlite` | `/data/crier.sqlite` in the image |
| `CRIER_POLL_INTERVAL_SECONDS` | `600` | |
| `CRIER_HN_TOP`, `CRIER_LOBSTERS_TOP` | `10` | |
| `CRIER_INITIAL_POSTS` | `3` | |
| `CRIER_MAX_POSTS_PER_CYCLE` | `10` | |

## Running

```sh
# Print what would be posted, without Telegram (use a throwaway database:
# stories printed here count as posted):
OPENAI_API_KEY=… cargo run -- once --dry-run --database-url sqlite:///tmp/crier.sqlite

# For real
TELEGRAM_BOT_TOKEN=… TELEGRAM_CHAT_ID=@mychannel OPENAI_API_KEY=… cargo run -- run
```

Telegram setup: create a bot with @BotFather, create a channel, add the bot
as an administrator with "Post messages" and "Edit messages of others".

## Deploying

`charts/crier` is a Helm chart: one Deployment (Recreate, SQLite on a small
PVC). It expects the image to come from [GitImage](https://github.com/DiverOfDark/gitimage)
via the `gitimage.dev/image.crier` pod annotation; set `image.repository` to
use a regular registry instead.
