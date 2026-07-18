use chrono::{DateTime, Utc};

pub fn relative_time(t: DateTime<Utc>) -> String {
    let delta = Utc::now().signed_duration_since(t);
    let (n, unit) = if delta.num_days() >= 1 {
        (delta.num_days(), "d")
    } else if delta.num_hours() >= 1 {
        (delta.num_hours(), "h")
    } else {
        (delta.num_minutes().max(0), "m")
    };
    format!("{n}{unit} ago")
}
