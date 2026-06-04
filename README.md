# 🐈 Labelbot

> _Meet **Tabby** 🐾 — the tabby cat who sorts your mail while you nap._
>
> ```
>      /\_/\
>     ( o.o )   ~ "another one for Labels/Finance, meow"
>      > ^ <
> ```

**AI-powered IMAP email classifier** — watches your inbox via IMAP IDLE, classifies each message using Anthropic Claude, and sorts them into `Labels/Work`, `Labels/Finance`, `Labels/Personal` etc. Right there on the server. No filters, no rules, no effort. 🧠✨

Built for **Proton Mail Bridge** 🛡️, works with any IMAP server that supports IDLE and server-side mailboxes.

---

## 🚀 How It Works

```
📨 New email arrives → 📥 IMAP IDLE wakes up
    → 🧠 Claude classifies subject + from into labels
    → 📂 IMAP COPY into Labels/Work, Labels/Finance, …
    → 🗄️ Record message_id in SQLite (no double-processing)
```

- 🟢 Connects via **STARTTLS**, sits in **IMAP IDLE** (zero polling)
- 🏷️ **Configurable labels** — defaults to `Work`, `Finance`, `Newsletters`, `Receipts`, `Personal`, `Travel`, `Shopping`, `Notifications`, `House`, `Family`; override via `[labels.*]` in `config.toml`
- ⭐ **Important flag** — mark any label `important = true` and emails tagged with it also get an auto-applied `Important` label
- 🔄 **Backfill** existing messages on first run (configurable age range)
- 📊 **SQLite dedup** — never classifies the same email twice
- 🛑 **Graceful shutdown** — finishes current email, exits cleanly on SIGTERM/SIGINT
- ⏳ **Exponential backoff** with jitter on rate limits (429s) and 5xx errors

## 📄 License

MIT
