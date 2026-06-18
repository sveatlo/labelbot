use chrono::{DateTime, Local};

fn main() {
    set_build_timestamp();
}

fn set_build_timestamp() {
    let build_timestamp =
        option_env!("GIT_TIMESTAMP").map_or("unknown".to_owned(), |timestamp_secs| {
            let timestamp_secs = timestamp_secs
                .parse::<i64>()
                .expect("BUG: failed to parse timestamp_secs");

            let utc = DateTime::from_timestamp(timestamp_secs, 0)
                .expect("BUG: failed to create NaiveDateTime from timestamp_secs");

            DateTime::<Local>::from(utc).to_rfc3339()
        });

    println!("cargo:rustc-env=GIT_TIMESTAMP={build_timestamp}");
}
