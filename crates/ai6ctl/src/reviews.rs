//! `ai6ctl reviews`: what a Mission's review panel decided and why, and a person's overrule,
//! through the gateway's routes. A refusal prints the gateway's reason.

use clap::Subcommand;
use reqwest::{Client, Method, Url};
use serde_json::{Value, json};

use crate::access::{exchange, text, url};

#[derive(Debug, Subcommand)]
pub(crate) enum ReviewsCommand {
    /// Show each gate's review of a Mission: the panel, every vote with its weight, model, round
    /// and reasons (needs mission.read).
    Show {
        /// The Mission's Operation.
        #[arg(long)]
        operation: String,
        /// The Mission ID.
        mission_id: String,
    },
    /// Decide over a Mission's panel, with a reason, audited (needs mission.review).
    Overrule {
        /// The Mission's Operation.
        #[arg(long)]
        operation: String,
        /// The Mission ID.
        mission_id: String,
        /// `plan` or `output`.
        #[arg(long, default_value = "output")]
        gate: String,
        /// `pass`, `redo` (the reason goes to the author) or `refuse` (the Mission fails).
        #[arg(long)]
        decision: String,
        /// Why.
        #[arg(long)]
        reason: String,
    },
}

pub(crate) async fn run(
    client: &Client,
    server: &Url,
    token: &str,
    command: ReviewsCommand,
) -> Result<(), String> {
    match command {
        ReviewsCommand::Show {
            operation,
            mission_id,
        } => {
            let reviews = exchange(
                client,
                token,
                Method::GET,
                url(
                    server,
                    &["operations", &operation, "missions", &mission_id, "reviews"],
                )?,
                None,
                None,
            )
            .await?;
            for review in reviews.as_array().into_iter().flatten() {
                print_review(review);
            }
        }
        ReviewsCommand::Overrule {
            operation,
            mission_id,
            gate,
            decision,
            reason,
        } => {
            let review = exchange(
                client,
                token,
                Method::POST,
                url(
                    server,
                    &[
                        "operations",
                        &operation,
                        "missions",
                        &mission_id,
                        "reviews",
                        &gate,
                        "overrule",
                    ],
                )?,
                None,
                Some(json!({ "decision": decision, "reason": reason })),
            )
            .await?;
            print_review(&review);
        }
    }
    Ok(())
}

fn print_review(review: &Value) {
    println!(
        "{}\t{}\t{}\titeration {}\tround {}\tsent back {}\tturns {}\ttokens {}",
        text(&review["gate"]),
        text(&review["policy"]),
        text(&review["status"]),
        review["iteration"],
        review["round"],
        review["sent_back"],
        review["turns"],
        review["tokens"],
    );
    if let Some(why) = review["why"].as_str() {
        println!("  why\t{}", crate::plain(why));
    }
    for seat in review["panel"].as_array().into_iter().flatten() {
        println!(
            "  seat {}\t{}\tweight {}{}\t{}",
            seat["seat"],
            text(&seat["agent"]),
            seat["weight"],
            if seat["veto"].as_bool() == Some(true) {
                " veto"
            } else {
                ""
            },
            text(&seat["model"]),
        );
    }
    for vote in review["votes"].as_array().into_iter().flatten() {
        let reasons = vote["reasons"]
            .as_array()
            .map(|reasons| reasons.iter().map(text).collect::<Vec<_>>().join("; "))
            .unwrap_or_default();
        println!(
            "  {}.{}\t{}\t{}\tweight {}\t{}\t{}",
            vote["iteration"],
            vote["round"],
            text(&vote["agent"]),
            text(&vote["vote"]),
            vote["weight"],
            text(&vote["model"]),
            reasons,
        );
    }
    if let Some(by) = review["overrule"]["by"].as_str() {
        println!(
            "  overruled by {}: {}, {}",
            crate::plain(by),
            text(&review["overrule"]["decision"]),
            text(&review["overrule"]["reason"]),
        );
    }
}
