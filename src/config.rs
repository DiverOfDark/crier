use clap::{Args, Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "crier", version, about)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Poll the sources forever, posting new stories as they appear.
    Run(Config),
    /// Run a single poll cycle and exit.
    Once(Config),
}

#[derive(Args, Debug, Clone)]
pub struct Config {
    /// SQLite database that remembers which stories were already handled.
    #[arg(
        long,
        env = "CRIER_DATABASE_URL",
        default_value = "sqlite://crier.sqlite"
    )]
    pub database_url: String,

    /// Seconds between poll cycles.
    #[arg(long, env = "CRIER_POLL_INTERVAL_SECONDS", default_value_t = 600)]
    pub poll_interval_seconds: u64,

    /// How many stories from the top of the Hacker News front page to consider.
    #[arg(long, env = "CRIER_HN_TOP", default_value_t = 10)]
    pub hn_top: usize,

    /// How many stories from the top of Lobsters "hottest" to consider.
    #[arg(long, env = "CRIER_LOBSTERS_TOP", default_value_t = 10)]
    pub lobsters_top: usize,

    /// On the very first cycle (empty database) post only this many stories
    /// and mark the rest of the current front pages as already seen.
    #[arg(long, env = "CRIER_INITIAL_POSTS", default_value_t = 3)]
    pub initial_posts: usize,

    /// Upper bound on posts per cycle, so a backlog drains gradually.
    #[arg(long, env = "CRIER_MAX_POSTS_PER_CYCLE", default_value_t = 10)]
    pub max_posts_per_cycle: usize,

    /// Language the posts are written in.
    #[arg(long, env = "CRIER_POST_LANGUAGE", default_value = "English")]
    pub post_language: String,

    /// OpenAI-compatible chat completions endpoint.
    #[arg(
        long,
        env = "CRIER_OPENAI_URL",
        default_value = "https://api.openai.com/v1/chat/completions"
    )]
    pub openai_url: String,

    #[arg(long, env = "CRIER_OPENAI_MODEL", default_value = "gpt-5-mini")]
    pub openai_model: String,

    /// Sent as `reasoning_effort` when set (OpenAI reasoning models).
    #[arg(long, env = "CRIER_OPENAI_REASONING_EFFORT")]
    pub openai_reasoning_effort: Option<String>,

    #[arg(long, env = "OPENAI_API_KEY", hide_env_values = true)]
    pub openai_api_key: String,

    #[arg(
        long,
        env = "TELEGRAM_BOT_TOKEN",
        hide_env_values = true,
        default_value = ""
    )]
    pub telegram_bot_token: String,

    /// Channel username (`@mychannel`) or numeric chat id (`-100…`).
    #[arg(long, env = "TELEGRAM_CHAT_ID", default_value = "")]
    pub telegram_chat_id: String,

    /// Print posts to stdout instead of sending them to Telegram.
    #[arg(long, env = "CRIER_DRY_RUN")]
    pub dry_run: bool,

    #[arg(long, env = "CRIER_LOG_JSON")]
    pub log_json: bool,

    #[arg(
        long,
        hide = true,
        default_value = "https://hacker-news.firebaseio.com/v0"
    )]
    pub hn_api_base: String,

    #[arg(long, hide = true, default_value = "https://lobste.rs")]
    pub lobsters_base: String,

    #[arg(long, hide = true, default_value = "https://api.telegram.org")]
    pub telegram_api_base: String,
}

impl Config {
    pub fn validate(&self) -> anyhow::Result<()> {
        if !self.dry_run {
            anyhow::ensure!(
                !self.telegram_bot_token.is_empty(),
                "TELEGRAM_BOT_TOKEN is required unless --dry-run is set"
            );
            anyhow::ensure!(
                !self.telegram_chat_id.is_empty(),
                "TELEGRAM_CHAT_ID is required unless --dry-run is set"
            );
        }
        anyhow::ensure!(
            self.poll_interval_seconds >= 30,
            "poll interval must be >= 30s"
        );
        Ok(())
    }
}
