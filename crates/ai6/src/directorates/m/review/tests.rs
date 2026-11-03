use super::*;

const ANY: Limit = Limit::at(4096);

fn ballot(vote: Vote, weight: u32, model: &str) -> Ballot {
    Ballot {
        vote,
        weight,
        veto: false,
        model: model.to_owned(),
    }
}

fn panel(seats: &[u32]) -> ReviewPolicy {
    ReviewPolicy::new(
        seats
            .iter()
            .enumerate()
            .map(|(index, weight)| PanelSeat::agent(format!("a{index}")).weighing(*weight))
            .collect(),
    )
}

fn at(pass_at: f64, seats: &[u32]) -> ReviewPolicy {
    ReviewPolicy {
        pass_at,
        ..panel(seats)
    }
}

#[test]
fn every_seat_must_have_voted_unless_a_veto_settles_it() {
    let policy = panel(&[1, 1]);
    assert_eq!(decide(&policy, 2, &[], 1), Decision::Waiting);
    assert_eq!(
        decide(&policy, 2, &[ballot(Vote::Pass, 1, "x")], 1),
        Decision::Waiting
    );
    let veto = Ballot {
        veto: true,
        ..ballot(Vote::Redo, 1, "x")
    };
    assert_eq!(decide(&policy, 2, &[veto], 1), Decision::Vetoed);
}

#[test]
fn a_veto_that_passes_or_abstains_vetoes_nothing() {
    let policy = panel(&[1, 1]);
    for vote in [Vote::Pass, Vote::Abstain, Vote::Absent] {
        let veto = Ballot {
            veto: true,
            ..ballot(vote, 1, "x")
        };
        assert_eq!(
            decide(&policy, 2, &[veto], 1),
            Decision::Waiting,
            "{vote:?}"
        );
    }
}

#[test]
fn a_veto_ends_the_round_however_heavy_the_passes() {
    let policy = at(0.5, &[100, 1]);
    let votes = [
        ballot(Vote::Pass, 100, "x"),
        Ballot {
            veto: true,
            ..ballot(Vote::Redo, 1, "y")
        },
    ];
    assert_eq!(decide(&policy, 2, &votes, 1), Decision::Vetoed);
}

#[test]
fn the_default_share_is_all_of_the_weight() {
    let policy = panel(&[2, 1]);
    assert_eq!(
        decide(
            &policy,
            2,
            &[ballot(Vote::Pass, 2, "x"), ballot(Vote::Pass, 1, "y")],
            1
        ),
        Decision::Pass
    );
    assert_eq!(
        decide(
            &policy,
            2,
            &[ballot(Vote::Pass, 2, "x"), ballot(Vote::Redo, 1, "y")],
            1
        ),
        Decision::NoAgreement,
        "neither reached all of the weight, in the only round"
    );
    assert_eq!(
        decide(
            &policy,
            2,
            &[ballot(Vote::Redo, 2, "x"), ballot(Vote::Redo, 1, "y")],
            1
        ),
        Decision::Redo
    );
}

#[test]
fn pass_at_is_met_exactly_at_its_edge_and_not_below() {
    let votes = [ballot(Vote::Pass, 2, "x"), ballot(Vote::Redo, 1, "y")];
    assert_eq!(decide(&at(0.66, &[2, 1]), 2, &votes, 1), Decision::Pass);
    assert_eq!(
        decide(&at(2.0 / 3.0, &[2, 1]), 2, &votes, 1),
        Decision::Pass,
        "exactly two thirds of the weight"
    );
    assert_eq!(
        decide(&at(0.67, &[2, 1]), 2, &votes, 1),
        Decision::NoAgreement
    );
    let half = [ballot(Vote::Pass, 1, "x"), ballot(Vote::Redo, 1, "y")];
    assert_eq!(
        decide(&at(0.5, &[1, 1]), 2, &half, 1),
        Decision::Pass,
        "both reach a low share, and the pass stands"
    );
}

#[test]
fn weights_decide_not_the_count_of_voters() {
    let policy = at(0.6, &[3, 1, 1]);
    assert_eq!(
        decide(
            &policy,
            3,
            &[
                ballot(Vote::Pass, 3, "x"),
                ballot(Vote::Redo, 1, "y"),
                ballot(Vote::Redo, 1, "z")
            ],
            1
        ),
        Decision::Pass,
        "one voter holding three fifths passes against two"
    );
    assert_eq!(
        decide(
            &policy,
            3,
            &[
                ballot(Vote::Redo, 3, "x"),
                ballot(Vote::Pass, 1, "y"),
                ballot(Vote::Pass, 1, "z")
            ],
            1
        ),
        Decision::Redo
    );
}

#[test]
fn an_abstention_leaves_its_weight_out_and_an_absence_keeps_it_in() {
    let policy = panel(&[1, 1]);
    assert_eq!(
        decide(
            &policy,
            2,
            &[ballot(Vote::Pass, 1, "x"), ballot(Vote::Abstain, 1, "y")],
            1
        ),
        Decision::Pass,
        "all of what is left passed"
    );
    assert_eq!(
        decide(
            &policy,
            2,
            &[ballot(Vote::Pass, 1, "x"), ballot(Vote::Absent, 1, "y")],
            1
        ),
        Decision::NoAgreement,
        "a reviewer that failed makes all of the weight unreachable"
    );
    assert_eq!(
        decide(
            &policy,
            2,
            &[ballot(Vote::Abstain, 1, "x"), ballot(Vote::Abstain, 1, "y")],
            1
        ),
        Decision::NoAgreement,
        "nothing judged, nothing reached"
    );
}

#[test]
fn an_absent_reviewer_cannot_be_turned_into_a_pass_by_a_lower_share_it_does_not_reach() {
    let policy = at(0.7, &[2, 1]);
    assert_eq!(
        decide(
            &policy,
            2,
            &[ballot(Vote::Pass, 1, "x"), ballot(Vote::Absent, 2, "y")],
            1
        ),
        Decision::NoAgreement
    );
    assert_eq!(
        decide(
            &policy,
            2,
            &[ballot(Vote::Pass, 2, "x"), ballot(Vote::Absent, 1, "y")],
            1
        ),
        Decision::NoAgreement,
        "two thirds of the weight is below 0.7"
    );
}

#[test]
fn a_pass_needs_its_models_among_those_who_judged() {
    let policy = ReviewPolicy {
        distinct_models: 2,
        ..panel(&[1, 1])
    };
    assert_eq!(
        decide(
            &policy,
            2,
            &[ballot(Vote::Pass, 1, "x"), ballot(Vote::Abstain, 1, "y")],
            1
        ),
        Decision::NoAgreement,
        "one model passing while the other abstains is one model deciding"
    );
    assert_eq!(
        decide(
            &policy,
            2,
            &[ballot(Vote::Pass, 1, "x"), ballot(Vote::Pass, 1, "x")],
            1
        ),
        Decision::NoAgreement,
        "two Agents on one model are one model"
    );
    assert_eq!(
        decide(
            &policy,
            2,
            &[ballot(Vote::Pass, 1, "x"), ballot(Vote::Pass, 1, "y")],
            1
        ),
        Decision::Pass
    );
}

#[test]
fn a_redo_does_not_need_enough_models() {
    let policy = ReviewPolicy {
        distinct_models: 2,
        ..panel(&[1, 1])
    };
    assert_eq!(
        decide(
            &policy,
            2,
            &[ballot(Vote::Redo, 1, "x"), ballot(Vote::Abstain, 1, "y")],
            1
        ),
        Decision::Redo
    );
}

#[test]
fn a_split_goes_to_the_next_round_until_there_are_none() {
    let policy = ReviewPolicy {
        rounds: 2,
        ..panel(&[1, 1])
    };
    let split = [ballot(Vote::Pass, 1, "x"), ballot(Vote::Redo, 1, "y")];
    assert_eq!(decide(&policy, 2, &split, 1), Decision::NextRound);
    assert_eq!(decide(&policy, 2, &split, 2), Decision::NoAgreement);
    assert_eq!(
        decide(&policy, 2, &split, 3),
        Decision::NoAgreement,
        "a round past the last is no better"
    );
}

#[test]
fn what_each_decision_does() {
    let policy = ReviewPolicy {
        redo_limit: 2,
        ..panel(&[1])
    };
    let step = |decision, sent_back| next_step(&policy, ReviewGate::Output, decision, sent_back);
    assert_eq!(step(Decision::Waiting, 0), Step::Wait);
    assert_eq!(
        step(Decision::Pass, 5),
        Step::Pass,
        "a pass stands at any count"
    );
    assert_eq!(step(Decision::NextRound, 0), Step::AskAgain);
    for decision in [Decision::Redo, Decision::Vetoed] {
        assert_eq!(step(decision, 0), Step::SendBack);
        assert_eq!(step(decision, 1), Step::SendBack);
        assert_eq!(step(decision, 2), Step::Person(REDO_REASON));
        assert_eq!(step(decision, 9), Step::Person(REDO_REASON));
    }
    assert_eq!(step(Decision::NoAgreement, 0), Step::Person(OUTPUT_REASON));
    assert_eq!(
        next_step(&policy, ReviewGate::Plan, Decision::NoAgreement, 0),
        Step::Person(PLAN_REASON)
    );
    let refusing = ReviewPolicy {
        on_no_agreement: OnNoAgreement::Refuse,
        ..policy.clone()
    };
    assert_eq!(
        next_step(&refusing, ReviewGate::Output, Decision::NoAgreement, 0),
        Step::Refuse
    );
    let never_back = ReviewPolicy {
        redo_limit: 0,
        ..policy
    };
    assert_eq!(
        next_step(&never_back, ReviewGate::Output, Decision::Redo, 0),
        Step::Person(REDO_REASON),
        "no redos allowed goes straight to a person"
    );
}

#[test]
fn a_budget_stops_the_next_turn_and_none_means_none() {
    let free = panel(&[1]);
    assert!(!out_of_budget(&free, u64::MAX, u64::MAX));
    let turns = ReviewPolicy {
        max_turns: Some(2),
        ..panel(&[1])
    };
    assert!(!out_of_budget(&turns, 1, u64::MAX));
    assert!(out_of_budget(&turns, 2, 0));
    let tokens = ReviewPolicy {
        max_tokens: Some(100),
        ..panel(&[1])
    };
    assert!(!out_of_budget(&tokens, u64::MAX, 99));
    assert!(out_of_budget(&tokens, 0, 100));
}

#[test]
fn what_no_setting_can_mean_is_refused_by_name() {
    let refused = |policy: ReviewPolicy, word: &str| {
        let reason = policy.validate().unwrap_err();
        assert!(reason.contains(word), "{reason}");
    };
    refused(ReviewPolicy::new(Vec::new()), "panel");
    let mut both = PanelSeat::agent("a");
    both.role = Some("r".to_owned());
    refused(ReviewPolicy::new(vec![both]), "seat 1");
    let nobody = PanelSeat {
        agent: None,
        role: None,
        weight: 1,
        veto: false,
    };
    refused(ReviewPolicy::new(vec![nobody]), "seat 1");
    refused(ReviewPolicy::new(vec![PanelSeat::agent("  ")]), "seat 1");
    refused(
        ReviewPolicy::new(vec![PanelSeat::agent("a").weighing(0)]),
        "no weight and no veto",
    );
    refused(
        ReviewPolicy::new(vec![PanelSeat::agent("a").weighing(0).with_veto()]),
        "no weight, so nothing can pass",
    );
    for pass_at in [0.0, -0.1, 1.01, f64::NAN] {
        refused(at(pass_at, &[1]), "pass_at");
    }
    refused(
        ReviewPolicy {
            rounds: 0,
            ..panel(&[1])
        },
        "rounds",
    );
    refused(
        ReviewPolicy {
            distinct_models: 0,
            ..panel(&[1])
        },
        "distinct_models",
    );
    assert_eq!(
        ReviewPolicy {
            redo_limit: 0,
            ..panel(&[1])
        }
        .validate(),
        Ok(()),
        "never sending it back is a choice"
    );
    assert_eq!(
        ReviewPolicy::new(vec![
            PanelSeat::agent("a"),
            PanelSeat::agent("b").weighing(0).with_veto()
        ])
        .validate(),
        Ok(()),
        "a seat with only a veto is a choice"
    );
}

#[test]
fn a_record_reads_as_written_and_a_bad_one_is_refused_by_name() {
    let policies = policies_from_records([(
        "two-models".to_owned(),
        serde_json::json!({
            "panel": [
                { "role": "reviewer", "weight": 2, "veto": true },
                { "agent": "Tanner" },
            ],
            "pass_at": 0.66,
            "distinct_models": 2,
            "rounds": 2,
            "redo_limit": 2,
            "on_no_agreement": "refuse",
            "max_turns": 12,
            "max_tokens": 1000,
        }),
    )])
    .unwrap();
    let policy = &policies["two-models"];
    assert_eq!(policy.panel[0].weight, 2);
    assert!(policy.panel[0].veto);
    assert_eq!(policy.panel[1].weight, 1, "weight defaults to 1");
    assert_eq!(policy.pass_at, 0.66);
    assert_eq!(policy.on_no_agreement, OnNoAgreement::Refuse);
    assert_eq!(policy.max_turns, Some(12));

    let defaults = policies_from_records([(
        "d".to_owned(),
        serde_json::json!({ "panel": [{"agent": "a"}] }),
    )])
    .unwrap();
    assert_eq!(
        defaults["d"],
        ReviewPolicy::new(vec![PanelSeat::agent("a")]),
        "every default as written in the module's constants"
    );

    let refused = |name: &str, definition: serde_json::Value, word: &str| {
        let reason = policies_from_records([(name.to_owned(), definition)]).unwrap_err();
        assert!(reason.contains(word) && reason.contains(name), "{reason}");
    };
    refused(
        "none",
        serde_json::json!({ "panel": [{"agent": "a"}] }),
        "means no review",
    );
    refused("empty", serde_json::json!({ "panel": [] }), "panel");
    refused(
        "unknown",
        serde_json::json!({ "panel": [{"agent": "a"}], "threshold": 1 }),
        "threshold",
    );
}

fn policies() -> BTreeMap<String, ReviewPolicy> {
    let two = ReviewPolicy {
        distinct_models: 2,
        ..ReviewPolicy::new(vec![PanelSeat::agent("a"), PanelSeat::agent("b")])
    };
    let one = ReviewPolicy::new(vec![PanelSeat::agent("a")]);
    let more = ReviewPolicy {
        redo_limit: 0,
        ..two.clone()
    };
    BTreeMap::from([
        ("one".to_owned(), one),
        ("two".to_owned(), two),
        ("two-stricter".to_owned(), more),
    ])
}

fn choice(output: &str) -> ReviewChoice {
    ReviewChoice {
        plan: None,
        output: Some(output.to_owned()),
    }
}

fn named(output: &str, authority: BriefAuthority) -> BriefNaming {
    BriefNaming::new(choice(output), authority)
}

struct Case<'a> {
    brief: Option<&'a BriefNaming>,
    role: Option<&'a ReviewChoice>,
    operation: Option<&'a ReviewChoice>,
    author: Option<&'a str>,
}

fn lookup(case: Case<'_>) -> Result<AppliedPolicy, ReviewRefusal> {
    let policies = policies();
    applicable(&PolicyLookup {
        brief: case.brief,
        role: case.role,
        operation: case.operation,
        author: case.author,
        ..PolicyLookup::new(ReviewGate::Output, &policies)
    })
}

#[test]
fn nothing_named_is_no_review() {
    let applied = lookup(Case {
        brief: None,
        role: None,
        operation: None,
        author: None,
    })
    .unwrap();
    assert_eq!(applied.name, None);
    assert_eq!(applied.from, PolicySource::Default);
}

#[test]
fn the_role_comes_before_the_operation() {
    let (operation, role) = (choice("two"), choice("one"));
    let applied = lookup(Case {
        brief: None,
        role: Some(&role),
        operation: Some(&operation),
        author: None,
    })
    .unwrap();
    assert_eq!(applied.name.as_deref(), Some("one"));
    assert_eq!(applied.from, PolicySource::Role);
    let applied = lookup(Case {
        brief: None,
        role: None,
        operation: Some(&operation),
        author: None,
    })
    .unwrap();
    assert_eq!(applied.name.as_deref(), Some("two"));
    assert_eq!(applied.from, PolicySource::Operation);
}

#[test]
fn a_gate_a_choice_leaves_out_is_taken_from_the_next() {
    let policies = policies();
    let operation = choice("two");
    let role = ReviewChoice {
        plan: Some("one".to_owned()),
        output: None,
    };
    let applied = applicable(&PolicyLookup {
        role: Some(&role),
        operation: Some(&operation),
        ..PolicyLookup::new(ReviewGate::Output, &policies)
    })
    .unwrap();
    assert_eq!(applied.name.as_deref(), Some("two"));
}

#[test]
fn a_label_cannot_turn_a_review_off_or_weaken_it() {
    let operation = choice("two");
    for weaker in [NO_REVIEW, "one"] {
        let naming = named(weaker, BriefAuthority::Outside);
        let applied = lookup(Case {
            brief: Some(&naming),
            role: None,
            operation: Some(&operation),
            author: None,
        })
        .unwrap();
        assert_eq!(applied.name.as_deref(), Some("two"), "{weaker}");
        assert_eq!(applied.from, PolicySource::Operation);
        assert_eq!(applied.weakens, None);
        let ignored = applied.ignored.expect("said to be ignored");
        assert_eq!(ignored.named, weaker);
        assert!(ignored.why.contains("only strengthen"), "{}", ignored.why);
    }
}

#[test]
fn a_label_can_strengthen() {
    let operation = choice("two");
    let naming = named("two-stricter", BriefAuthority::Outside);
    let applied = lookup(Case {
        brief: Some(&naming),
        role: None,
        operation: Some(&operation),
        author: None,
    })
    .unwrap();
    assert_eq!(applied.name.as_deref(), Some("two-stricter"));
    assert_eq!(applied.from, PolicySource::Mission);
    assert_eq!(applied.weakens, None);

    let naming = named("one", BriefAuthority::Outside);
    let applied = lookup(Case {
        brief: Some(&naming),
        role: None,
        operation: None,
        author: None,
    })
    .unwrap();
    assert_eq!(applied.name.as_deref(), Some("one"), "any review over none");
}

#[test]
fn a_label_naming_a_policy_that_does_not_exist_stops_nothing() {
    let operation = choice("two");
    let naming = named("nonesuch", BriefAuthority::Outside);
    let applied = lookup(Case {
        brief: Some(&naming),
        role: None,
        operation: Some(&operation),
        author: None,
    })
    .unwrap();
    assert_eq!(applied.name.as_deref(), Some("two"));
    assert!(applied.ignored.is_some());
}

#[test]
fn a_person_may_weaken_and_it_is_said_to_be_a_weakening() {
    let operation = choice("two");
    let naming = named(NO_REVIEW, BriefAuthority::Person);
    let applied = lookup(Case {
        brief: Some(&naming),
        role: None,
        operation: Some(&operation),
        author: None,
    })
    .unwrap();
    assert_eq!(applied.name, None);
    assert_eq!(applied.weakens.as_deref(), Some("two"));
    assert_eq!(applied.from, PolicySource::Mission);

    let naming = named("one", BriefAuthority::Person);
    let applied = lookup(Case {
        brief: Some(&naming),
        role: None,
        operation: Some(&operation),
        author: None,
    })
    .unwrap();
    assert_eq!(applied.name.as_deref(), Some("one"));
    assert_eq!(applied.weakens.as_deref(), Some("two"));
}

#[test]
fn a_person_strengthening_is_no_weakening() {
    let operation = choice("two");
    let naming = named("two-stricter", BriefAuthority::Person);
    let applied = lookup(Case {
        brief: Some(&naming),
        role: None,
        operation: Some(&operation),
        author: None,
    })
    .unwrap();
    assert_eq!(applied.weakens, None);
}

#[test]
fn a_zero_zero_agent_may_weaken_another_agents_mission_but_not_its_own() {
    let operation = choice("two");
    let naming = named(NO_REVIEW, BriefAuthority::ZeroZero("007".to_owned()));
    let other = lookup(Case {
        brief: Some(&naming),
        role: None,
        operation: Some(&operation),
        author: Some("008"),
    })
    .unwrap();
    assert_eq!(other.name, None);
    assert_eq!(other.weakens.as_deref(), Some("two"));

    let own = lookup(Case {
        brief: Some(&naming),
        role: None,
        operation: Some(&operation),
        author: Some("007"),
    })
    .unwrap();
    assert_eq!(own.name.as_deref(), Some("two"));
    assert!(own.ignored.unwrap().why.contains("its own"));

    let stronger = named("two-stricter", BriefAuthority::ZeroZero("007".to_owned()));
    let own = lookup(Case {
        brief: Some(&stronger),
        role: None,
        operation: Some(&operation),
        author: Some("007"),
    })
    .unwrap();
    assert_eq!(
        own.name.as_deref(),
        Some("two-stricter"),
        "it may strengthen its own"
    );
}

#[test]
fn a_policy_that_does_not_exist_is_refused_unless_outside_text_named_it() {
    let bad = choice("nonesuch");
    let refusal = lookup(Case {
        brief: None,
        role: None,
        operation: Some(&bad),
        author: None,
    })
    .unwrap_err();
    assert!(refusal.to_string().contains("the Operation"), "{refusal}");
    let refusal = lookup(Case {
        brief: None,
        role: Some(&bad),
        operation: None,
        author: None,
    })
    .unwrap_err();
    assert!(refusal.to_string().contains("role"), "{refusal}");
    let naming = named("nonesuch", BriefAuthority::Person);
    let refusal = lookup(Case {
        brief: Some(&naming),
        role: None,
        operation: None,
        author: None,
    })
    .unwrap_err();
    assert!(refusal.to_string().contains("the brief"), "{refusal}");
}

#[test]
fn strictness_compares_every_setting_that_can_let_more_through() {
    let base = ReviewPolicy {
        pass_at: 0.5,
        distinct_models: 2,
        rounds: 2,
        redo_limit: 2,
        max_turns: Some(10),
        max_tokens: Some(1000),
        ..ReviewPolicy::new(vec![
            PanelSeat::agent("a").weighing(2).with_veto(),
            PanelSeat::role("r"),
        ])
    };
    assert!(base.at_least_as_strict_as(&base));
    let weaker = |change: &dyn Fn(&mut ReviewPolicy), why: &str| {
        let mut changed = base.clone();
        change(&mut changed);
        assert!(!changed.at_least_as_strict_as(&base), "{why}");
    };
    weaker(&|p| p.pass_at = 0.4, "a lower share");
    weaker(&|p| p.distinct_models = 1, "fewer models");
    weaker(&|p| p.rounds = 3, "more chances to pass");
    weaker(&|p| p.redo_limit = 3, "more redos before a person");
    weaker(&|p| p.max_turns = None, "no budget");
    weaker(&|p| p.max_turns = Some(11), "a larger budget");
    weaker(&|p| p.max_tokens = None, "no token budget");
    weaker(&|p| p.panel.remove(1).veto = false, "a seat removed");
    weaker(&|p| p.panel[0].veto = false, "a veto removed");
    weaker(&|p| p.panel[0].weight = 1, "a seat's weight changed");
    weaker(&|p| p.panel[1].weight = 50, "a seat grown past the others");
    weaker(
        &|p| p.panel.push(PanelSeat::agent("x").weighing(100)),
        "a heavy seat that can carry a share below all of it",
    );
    let stricter = |change: &dyn Fn(&mut ReviewPolicy), why: &str| {
        let mut changed = base.clone();
        change(&mut changed);
        assert!(changed.at_least_as_strict_as(&base), "{why}");
    };
    stricter(&|p| p.pass_at = 0.9, "a higher share");
    stricter(&|p| p.distinct_models = 3, "more models");
    stricter(&|p| p.rounds = 1, "fewer rounds");
    stricter(&|p| p.redo_limit = 0, "no redos");
    stricter(&|p| p.on_no_agreement = OnNoAgreement::Refuse, "refusing");
    stricter(&|p| p.panel[1].veto = true, "a veto added");
    stricter(
        &|p| {
            p.pass_at = 1.0;
            p.panel.push(PanelSeat::agent("x").weighing(100));
        },
        "a seat added when all of the weight must pass",
    );
    let refusing = ReviewPolicy {
        on_no_agreement: OnNoAgreement::Refuse,
        ..base.clone()
    };
    assert!(!base.at_least_as_strict_as(&refusing), "person over refuse");
}

fn candidate(id: &str, roles: &[&str], models: &[&str]) -> PanelCandidate {
    PanelCandidate::new(id, roles, models, None)
}

#[test]
fn a_seat_takes_the_agent_it_names_or_one_with_the_role() {
    let policy = ReviewPolicy::new(vec![
        PanelSeat::role("reviewer").weighing(2).with_veto(),
        PanelSeat::agent("Tanner"),
    ]);
    let author = candidate("007", &["case-officer"], &["m0"]);
    let panel = choose_panel(
        &policy,
        &author,
        &[
            candidate("r1", &["reviewer"], &["m1"]),
            candidate("Tanner", &[], &["m2"]),
            author.clone(),
        ],
    )
    .unwrap();
    assert_eq!(panel.len(), 2);
    assert_eq!(panel[0].agent, "r1");
    assert_eq!(panel[0].role.as_deref(), Some("reviewer"));
    assert_eq!(panel[0].weight, 2);
    assert!(panel[0].veto);
    assert_eq!(panel[1].agent, "Tanner");
    assert_eq!(panel[1].role, None);
}

#[test]
fn a_sub_role_is_filled_by_an_agent_qualified_for_its_parent() {
    let policy = ReviewPolicy::new(vec![PanelSeat::role("reviewer/security")]);
    let author = candidate("007", &[], &["m0"]);
    let panel = choose_panel(&policy, &author, &[candidate("r", &["reviewer"], &["m1"])]).unwrap();
    assert_eq!(panel[0].agent, "r");
}

#[test]
fn the_author_cannot_sit_on_its_own_panel() {
    let policy = ReviewPolicy::new(vec![PanelSeat::agent("007")]);
    let author = candidate("007", &[], &["m0"]);
    let refusal = choose_panel(&policy, &author, std::slice::from_ref(&author)).unwrap_err();
    assert!(refusal.to_string().contains("wrote the work"), "{refusal}");
}

#[test]
fn nobody_on_the_authors_model_may_review_even_when_it_is_a_route() {
    let policy = ReviewPolicy::new(vec![PanelSeat::role("r")]);
    let author = candidate("007", &[], &["m0", "m0-bedrock"]);
    let refusal = choose_panel(
        &policy,
        &author,
        &[candidate("twin", &["r"], &["m0-bedrock", "other"])],
    )
    .unwrap_err();
    assert!(refusal.to_string().contains("author's model"), "{refusal}");
}

#[test]
fn an_agent_joined_to_the_authors_model_through_another_is_out() {
    let policy = ReviewPolicy::new(vec![PanelSeat::agent("far")]);
    let author = candidate("007", &[], &["m0"]);
    let refusal = choose_panel(
        &policy,
        &author,
        &[
            candidate("bridge", &[], &["m0", "m1"]),
            candidate("far", &[], &["m1"]),
        ],
    )
    .unwrap_err();
    assert!(refusal.to_string().contains("model"), "{refusal}");
}

#[test]
fn an_unavailable_agent_is_passed_over_and_its_reason_shown_when_no_one_is_left() {
    let policy = ReviewPolicy::new(vec![PanelSeat::role("r")]);
    let author = candidate("007", &[], &["m0"]);
    let quarantined = PanelCandidate::new("q", &["r"], &["m1"], Some("quarantined"));
    let panel = choose_panel(
        &policy,
        &author,
        &[quarantined.clone(), candidate("ok", &["r"], &["m2"])],
    )
    .unwrap();
    assert_eq!(panel[0].agent, "ok");
    let refusal = choose_panel(&policy, &author, &[quarantined]).unwrap_err();
    assert_eq!(
        refusal.to_string(),
        "no Agent can sit in role r: q: quarantined"
    );
}

#[test]
fn distinct_models_chooses_past_a_first_candidate_on_a_model_already_used() {
    let policy = ReviewPolicy {
        distinct_models: 2,
        ..ReviewPolicy::new(vec![PanelSeat::role("r"), PanelSeat::role("r")])
    };
    let author = candidate("007", &[], &["m0"]);
    let panel = choose_panel(
        &policy,
        &author,
        &[
            candidate("a", &["r"], &["m1"]),
            candidate("b", &["r"], &["m1"]),
            candidate("c", &["r"], &["m2"]),
        ],
    )
    .unwrap();
    assert_eq!(panel[0].agent, "a");
    assert_eq!(panel[1].agent, "c");
    assert_ne!(panel[0].model, panel[1].model);
}

#[test]
fn distinct_models_backs_out_of_a_seat_that_would_leave_another_empty() {
    let policy = ReviewPolicy {
        distinct_models: 2,
        ..ReviewPolicy::new(vec![PanelSeat::role("wide"), PanelSeat::role("narrow")])
    };
    let author = candidate("007", &[], &["m0"]);
    let panel = choose_panel(
        &policy,
        &author,
        &[
            candidate("both", &["wide", "narrow"], &["m1"]),
            candidate("wide-only", &["wide"], &["m2"]),
        ],
    )
    .unwrap();
    assert_eq!(panel[0].agent, "wide-only");
    assert_eq!(panel[1].agent, "both");
}

#[test]
fn too_few_models_says_how_many_it_found_and_never_reviews_with_fewer() {
    let policy = ReviewPolicy {
        distinct_models: 3,
        ..ReviewPolicy::new(vec![
            PanelSeat::role("r"),
            PanelSeat::role("r"),
            PanelSeat::role("r"),
        ])
    };
    let author = candidate("007", &[], &["m0"]);
    let refusal = choose_panel(
        &policy,
        &author,
        &[
            candidate("a", &["r"], &["m1"]),
            candidate("b", &["r"], &["m1"]),
            candidate("c", &["r"], &["m2"]),
        ],
    )
    .unwrap_err();
    assert_eq!(
        refusal,
        PanelRefusal::TooFewModels {
            wanted: 3,
            found: 2
        }
    );
}

#[test]
fn routes_that_share_a_name_count_as_one_model_across_agents() {
    let policy = ReviewPolicy {
        distinct_models: 2,
        ..ReviewPolicy::new(vec![PanelSeat::agent("a"), PanelSeat::agent("b")])
    };
    let author = candidate("007", &[], &["m0"]);
    let refusal = choose_panel(
        &policy,
        &author,
        &[
            candidate("a", &[], &["m1", "shared"]),
            candidate("b", &[], &["shared", "m2"]),
        ],
    )
    .unwrap_err();
    assert_eq!(
        refusal,
        PanelRefusal::TooFewModels {
            wanted: 2,
            found: 1
        }
    );
}

#[test]
fn an_agent_sits_once_however_many_seats_it_could_fill() {
    let policy = ReviewPolicy::new(vec![PanelSeat::role("r"), PanelSeat::role("r")]);
    let author = candidate("007", &[], &["m0"]);
    let refusal = choose_panel(&policy, &author, &[candidate("a", &["r"], &["m1"])]).unwrap_err();
    assert!(matches!(refusal, PanelRefusal::NoAgent { .. }), "{refusal}");
    assert!(refusal.to_string().contains("fewer Agents"), "{refusal}");
}

#[test]
fn an_agent_naming_no_model_counts_as_its_handlers_default() {
    let mut agent = Agent::new("a");
    agent.handler = Some("claude".to_owned());
    assert_eq!(
        models_of(&agent),
        BTreeSet::from(["handler:claude".to_owned()])
    );
    agent.model = Some("opus".to_owned());
    assert_eq!(models_of(&agent), BTreeSet::from(["opus".to_owned()]));
    let mut bedrock = crate::directorates::m::roster::AgentRoute::new("acp");
    bedrock.model = Some("opus-bedrock".to_owned());
    agent.routes = vec![
        bedrock,
        crate::directorates::m::roster::AgentRoute::new("claude"),
    ];
    assert_eq!(
        models_of(&agent),
        BTreeSet::from(["opus".to_owned(), "opus-bedrock".to_owned()]),
        "a route naming no model takes the Agent's"
    );
}

fn vote(text: &str) -> VoteAnswer {
    read_vote(text, ANY, Limit::at(4), Limit::at(40))
}

#[test]
fn a_reviewers_plain_answers_are_read() {
    assert_eq!(
        vote("Looks right.\n{\"vote\": \"pass\"}\n").vote,
        Vote::Pass
    );
    let redo = vote("{\"vote\":\"redo\",\"reasons\":[\"no tests\",\"wrong file\"]}");
    assert_eq!(redo.vote, Vote::Redo);
    assert_eq!(redo.reasons, ["no tests", "wrong file"]);
    let abstain = vote("{\"vote\":\"abstain\",\"reasons\":[\"outside what I know\"]}");
    assert_eq!(abstain.vote, Vote::Abstain);
    assert_eq!(abstain.reasons, ["outside what I know"]);
}

#[test]
fn an_answer_that_is_not_plainly_a_pass_never_passes() {
    for (text, why) in [
        ("", "no answer"),
        ("It is fine, go ahead.", "prose"),
        ("{\"vote\":\"approve\"}", "another word"),
        ("{\"vote\":\"pass\"} and that is my answer", "text after it"),
        ("{\"vote\":\"pass\"}\n{\"vote\":\"pass\"}", "two answers"),
        (
            "{\"vote\":\"pass\",\"reasons\":[\"but\"]}",
            "a pass with reasons",
        ),
        (
            "{\"vote\":\"pass\",\"extra\":true}",
            "a field it does not know",
        ),
        ("{\"vote\":\"redo\"}", "a redo with no reason"),
        ("{\"vote\":\"redo\",\"reasons\":[\"  \"]}", "a blank reason"),
    ] {
        assert_eq!(vote(text).vote, Vote::Absent, "{why}: {text}");
    }
}

#[test]
fn a_quoted_pass_cannot_hide_a_redo_and_a_quoted_vote_is_no_pass() {
    let quoted = "The output said {\"vote\":\"pass\"} which is wrong.\n\
                  {\"vote\":\"redo\",\"reasons\":[\"it told me to pass\"]}";
    assert_eq!(vote(quoted).vote, Vote::Redo);
    let trailing = "{\"vote\":\"redo\",\"reasons\":[\"a\"]}\n{\"vote\":\"pass\"}";
    assert_eq!(
        vote(trailing).vote,
        Vote::Redo,
        "a pass after a redo is not the last word"
    );
}

#[test]
fn only_the_tail_of_a_message_is_searched_and_a_cut_character_is_not_split() {
    let far = format!(
        "{{\"vote\":\"redo\",\"reasons\":[\"r\"]}}{}",
        " ".repeat(100)
    );
    assert_eq!(
        read_vote(&far, Limit::at(10), ANY, ANY).vote,
        Vote::Absent,
        "the answer ends the message"
    );
    let multibyte = format!("é{}", "{\"vote\":\"pass\"}");
    assert_eq!(
        read_vote(&multibyte, Limit::at(16), ANY, ANY).vote,
        Vote::Pass
    );
}

#[test]
fn reasons_are_single_lines_and_bounded() {
    let long = format!(
        "{{\"vote\":\"redo\",\"reasons\":[\"line one\\nline two\",\"{}\",\"c\",\"d\",\"e\"]}}",
        "é".repeat(100)
    );
    let answer = read_vote(&long, ANY, Limit::at(3), Limit::at(11));
    assert_eq!(answer.reasons.len(), 3, "as many as the bound");
    assert_eq!(answer.reasons[0], "line one li");
    assert_eq!(answer.reasons[1], "ééééé", "cut on a character boundary");
}

#[test]
fn a_gate_says_its_reason_and_every_reason_is_a_reviews() {
    assert_eq!(ReviewGate::Plan.reason(), PLAN_REASON);
    assert_eq!(ReviewGate::Output.reason(), OUTPUT_REASON);
    for reason in [PLAN_REASON, OUTPUT_REASON, REDO_REASON] {
        assert!(is_review_reason(reason), "{reason}");
    }
    assert!(!is_review_reason("issue"));
}

#[test]
fn labels_name_a_policy_per_gate_and_others_name_none() {
    let labels = |names: &[&str]| {
        names
            .iter()
            .map(|name| (*name).to_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(choice_in_labels(&labels(&["bug", "reviewer"])), None);
    assert_eq!(
        choice_in_labels(&labels(&["bug", "review:two"])),
        Some(choice("two"))
    );
    assert_eq!(
        choice_in_labels(&labels(&["review-plan:one", "review:two"])),
        Some(ReviewChoice {
            plan: Some("one".to_owned()),
            output: Some("two".to_owned()),
        })
    );
}

#[test]
fn a_label_saying_none_is_outside_text_like_any_other() {
    let policies = policies();
    let operation = choice("two");
    let labels = vec!["review:none".to_owned()];
    let naming = BriefNaming::new(choice_in_labels(&labels).unwrap(), BriefAuthority::Outside);
    let applied = applicable(&PolicyLookup {
        brief: Some(&naming),
        operation: Some(&operation),
        ..PolicyLookup::new(ReviewGate::Output, &policies)
    })
    .unwrap();
    assert_eq!(applied.name.as_deref(), Some("two"));
}

#[test]
fn a_brief_names_a_policy_in_its_data_and_a_bad_shape_names_none() {
    let data = |value: serde_json::Value| BTreeMap::from([(BRIEF_REVIEW_KEY.to_owned(), value)]);
    assert_eq!(
        choice_in_data(&data(serde_json::json!({ "output": "two" }))),
        Some(choice("two"))
    );
    assert_eq!(choice_in_data(&BTreeMap::new()), None);
    assert_eq!(choice_in_data(&data(serde_json::json!({}))), None);
    assert_eq!(choice_in_data(&data(serde_json::json!("two"))), None);
    assert_eq!(
        choice_in_data(&data(serde_json::json!({ "output": 7 }))),
        None
    );
}
