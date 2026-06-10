// SPDX-License-Identifier: MIT
//! Manual smoke test: fetch live quota for all providers and print
//! percentages/statuses. Never prints credentials.

#[tokio::main(flavor = "current_thread")]
async fn main() {
    for snapshot in quota_providers::fetch_all().await {
        println!("{}: {:?}", snapshot.provider.name(), snapshot.status);
        if let Some(plan) = &snapshot.plan {
            println!("  plan: {plan}");
        }
        for window in &snapshot.windows {
            println!(
                "  {:<14} {:>5.1}%  resets_at={:?}",
                window.label, window.used_percent, window.resets_at
            );
        }
    }
}
