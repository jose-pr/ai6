use std::collections::{BTreeMap, BTreeSet};

use super::*;

/// ROOT > P > X, and ROOT > S > Q.
fn tree() -> Lineage {
    Lineage::new([
        ("ROOT", vec![]),
        ("P", vec!["ROOT"]),
        ("X", vec!["P", "ROOT"]),
        ("S", vec!["ROOT"]),
        ("Q", vec!["S", "ROOT"]),
    ])
}

fn person(name: &str, groups: &[&str]) -> Person {
    Person {
        id: format!("person-{name}"),
        name: name.to_owned(),
        groups: groups.iter().map(|group| (*group).to_owned()).collect(),
        ..Person::default()
    }
}

/// The seeded roles, `ctl` as Controller for P, `boss` as chief.
fn global() -> AccessPolicy {
    let mut policy = seeded_policy();
    policy
        .rules
        .push(RuleDef::new("account:ctl", "operation:P", "controller"));
    policy.chief = vec!["account:boss".into()];
    policy
}

fn request(who: &Person, action: &str, operation: &str, lineage: &Lineage) -> AuthorizationRequest {
    AuthorizationRequest {
        person: who.clone(),
        action: action.to_owned(),
        resource: format!("operation:{operation}"),
        ancestors: lineage
            .ancestors(operation)
            .iter()
            .map(|ancestor| format!("operation:{ancestor}"))
            .collect(),
    }
}

fn is_allowed(
    access: &EffectiveAccess,
    who: &Person,
    action: &str,
    operation: &str,
    lineage: &Lineage,
) -> bool {
    access
        .authorize(request(who, action, operation, lineage))
        .unwrap()
        .allowed
}

fn record(roles: &[(&str, &[&str])], rules: &[(&str, &str, &str)]) -> OperationAccess {
    OperationAccess {
        roles: roles
            .iter()
            .map(|(name, actions)| ((*name).to_owned(), RoleDef::new(actions.iter().copied())))
            .collect(),
        rules: rules
            .iter()
            .map(|(subject, scope, role)| RuleDef::new(*subject, *scope, *role))
            .collect(),
    }
}

type Records = BTreeMap<String, OperationAccess>;

fn change(
    policy: &AccessPolicy,
    records: &Records,
    lineage: &Lineage,
    who: &Person,
    operation: &str,
    after: &OperationAccess,
) -> Result<EffectiveAccess, Refusal> {
    let effective = compile_with_operations(policy, &[], records, lineage).unwrap();
    let editor = Editor::new(who.clone(), &effective);
    let set = AccessSet {
        global: policy,
        operations: records,
        lineage,
    };
    let before = records.get(operation).cloned().unwrap_or_default();
    check_operation_change(operation, &before, after, &editor, &effective, &set)?;
    let mut next = records.clone();
    next.insert(operation.to_owned(), after.clone());
    Ok(compile_with_operations(policy, &[], &next, lineage).unwrap())
}

#[test]
fn a_controller_for_p_grants_inside_p_and_nowhere_else() {
    let (lineage, policy) = (tree(), global());
    let after = record(
        &[("reviewer", &["mission.read", "approval.decide"])],
        &[("group:qa", "operation:X", "reviewer")],
    );
    let access = change(
        &policy,
        &Records::new(),
        &lineage,
        &person("ctl", &[]),
        "P",
        &after,
    )
    .unwrap();
    let qa = person("q1", &["qa"]);
    assert!(is_allowed(&access, &qa, "approval.decide", "X", &lineage));
    assert!(is_allowed(&access, &qa, "mission.read", "X", &lineage));
    assert!(!is_allowed(&access, &qa, "approval.decide", "S", &lineage));
    assert!(!is_allowed(&access, &qa, "approval.decide", "P", &lineage));
    assert!(!is_allowed(&access, &qa, "mission.create", "X", &lineage));
    let why = access
        .explain(&request(&qa, "approval.decide", "X", &lineage))
        .unwrap();
    assert_eq!(
        (why.tier, why.rule_index, why.role),
        (
            Some(TierKind::Operation("P".into())),
            Some(0),
            Some("reviewer".into())
        )
    );
}

#[test]
fn a_controller_for_p_is_refused_what_it_may_not_hand_out() {
    let (lineage, policy) = (tree(), global());
    let ctl = person("ctl", &[]);
    let refuse = |after: OperationAccess| {
        change(&policy, &Records::new(), &lineage, &ctl, "P", &after)
            .err()
            .map(|refusal| refusal.reason)
    };
    let bound = |role: &str, actions: &[&str]| {
        record(&[(role, actions)], &[("group:g", "operation:X", role)])
    };
    assert!(
        refuse(bound("r", &["station.configure"]))
            .unwrap()
            .contains("not an Operation action")
    );
    assert!(
        refuse(bound("r", &["approval.class"]))
            .unwrap()
            .contains("not delegable")
    );
    assert!(
        refuse(bound("r", &["access.manage"]))
            .unwrap()
            .contains("not delegable")
    );
    assert!(
        refuse(record(
            &[("r", &["mission.read"])],
            &[("group:g", "*", "r")]
        ))
        .unwrap()
        .contains("is not P or an Operation under it")
    );
    assert!(
        refuse(record(
            &[("r", &["mission.read"])],
            &[("group:g", "operation:S", "r")]
        ))
        .is_some(),
        "a sibling's scope"
    );
    assert!(
        refuse(record(
            &[("r", &["mission.read"])],
            &[("group:g", "station:hq", "r")]
        ))
        .is_some()
    );
    for built_in in ["operator", "controller", "auditor", "chief"] {
        assert!(
            refuse(bound(built_in, &["mission.read"]))
                .unwrap()
                .contains("built-in"),
            "{built_in}"
        );
    }
    assert!(
        refuse(record(&[], &[("group:g", "operation:X", "controller")]))
            .unwrap()
            .contains("not delegable"),
        "the default cap keeps controller (access.manage, approval.class) from an Operation record"
    );
    let ok = record(&[], &[("group:g", "operation:X", "operator")]);
    assert_eq!(refuse(ok), None, "operator's actions are all delegable");

    let mut thin = policy.clone();
    thin.roles
        .get_mut("controller")
        .unwrap()
        .actions
        .retain(|action| action != "pool.manage");
    let after = bound("r", &["pool.manage"]);
    let refused = change(&thin, &Records::new(), &lineage, &ctl, "P", &after)
        .err()
        .unwrap();
    assert!(refused.reason.contains("pool.manage"), "{refused}");
    let editor_elsewhere = person("nobody", &[]);
    let refused = change(
        &policy,
        &Records::new(),
        &lineage,
        &editor_elsewhere,
        "P",
        &after,
    )
    .err()
    .unwrap();
    assert!(refused.reason.contains("access.manage"));
    let at_sibling = change(&policy, &Records::new(), &lineage, &ctl, "S", &after)
        .err()
        .unwrap();
    assert!(at_sibling.reason.contains("access.manage"), "{at_sibling}");
}

#[test]
fn a_deputy_manages_the_operation_but_cannot_grant_what_it_lacks() {
    let lineage = tree();
    let mut policy = global();
    let mut cap = default_delegable();
    cap.push("access.manage".into());
    policy.delegable = Some(cap);
    let ctl = person("ctl", &[]);
    let deputy_record = record(
        &[("deputy", &["access.manage", "mission.read"])],
        &[("account:dep", "operation:X", "deputy")],
    );
    let access = change(
        &policy,
        &Records::new(),
        &lineage,
        &ctl,
        "P",
        &deputy_record,
    )
    .unwrap();
    let dep = person("dep", &[]);
    assert!(is_allowed(&access, &dep, "access.manage", "X", &lineage));
    assert!(!is_allowed(&access, &dep, "access.manage", "P", &lineage));

    let records: Records = [("P".to_owned(), deputy_record)].into();
    let grant = |action: &str| {
        let role = record(&[("r", &[action])], &[("group:g", "operation:X", "r")]);
        change(&policy, &records, &lineage, &dep, "X", &role)
            .err()
            .map(|refusal| refusal.reason)
    };
    assert!(
        grant("mission.control")
            .unwrap()
            .contains("mission.control")
    );
    assert_eq!(grant("mission.read"), None);
    let at_p = record(
        &[("r", &["mission.read"])],
        &[("group:g", "operation:P", "r")],
    );
    assert!(change(&policy, &records, &lineage, &dep, "P", &at_p).is_err());
}

#[test]
fn an_operation_policy_adds_to_a_global_rule_that_would_shadow_it() {
    let lineage = tree();
    let mut policy = global();
    policy
        .rules
        .insert(0, RuleDef::new("group:staff", "*", "auditor"));
    let records: Records = [(
        "X".to_owned(),
        record(
            &[("maker", &["mission.create"])],
            &[("group:staff", "operation:X", "maker")],
        ),
    )]
    .into();
    let access = compile_with_operations(&policy, &[], &records, &lineage).unwrap();
    let staff = person("s1", &["staff"]);
    assert!(is_allowed(&access, &staff, "mission.create", "X", &lineage));
    assert!(!is_allowed(
        &access,
        &staff,
        "mission.create",
        "S",
        &lineage
    ));
    assert!(is_allowed(&access, &staff, "mission.read", "X", &lineage));
}

#[test]
fn moving_an_operation_orphans_the_rules_that_no_longer_reach_it() {
    let policy = global();
    let records: Records = [(
        "P".to_owned(),
        record(
            &[("viewer", &["mission.read"])],
            &[
                ("group:g", "operation:X", "viewer"),
                ("group:g", "operation:P", "viewer"),
            ],
        ),
    )]
    .into();
    let moved = Lineage::new([
        ("ROOT", vec![]),
        ("P", vec!["ROOT"]),
        ("X", vec!["ROOT"]),
        ("S", vec!["ROOT"]),
    ]);
    let access = compile_with_operations(&policy, &[], &records, &moved).unwrap();
    let orphans = &access.orphaned()["P"];
    assert_eq!(orphans.len(), 1);
    assert_eq!(orphans[0].item, "rule 0");
    let g = person("g1", &["g"]);
    assert!(!is_allowed(&access, &g, "mission.read", "X", &moved));
    assert!(is_allowed(&access, &g, "mission.read", "P", &moved));

    let gone = Lineage::new([("ROOT", Vec::<&str>::new())]);
    let access = compile_with_operations(&policy, &[], &records, &gone).unwrap();
    assert_eq!(access.orphaned()["P"][0].item, "record");
}

#[test]
fn a_changed_cap_orphans_what_it_no_longer_allows_and_keeps_the_rest() {
    let lineage = tree();
    let mut policy = global();
    let records: Records = [(
        "P".to_owned(),
        record(
            &[("pooler", &["pool.manage"]), ("viewer", &["mission.read"])],
            &[
                ("group:g", "operation:P", "pooler"),
                ("group:g", "operation:P", "viewer"),
            ],
        ),
    )]
    .into();
    policy.delegable = Some(vec!["mission.read".into()]);
    let access = compile_with_operations(&policy, &[], &records, &lineage).unwrap();
    let items: Vec<&str> = access.orphaned()["P"]
        .iter()
        .map(|orphan| orphan.item.as_str())
        .collect();
    assert_eq!(items, ["role pooler", "rule 0"]);
    let g = person("g1", &["g"]);
    assert!(is_allowed(&access, &g, "mission.read", "P", &lineage));
    assert!(!is_allowed(&access, &g, "pool.manage", "P", &lineage));
}

#[test]
fn a_role_of_an_ancestor_resolves_below_it_and_its_own_shadows_it() {
    let lineage = tree();
    let policy = global();
    let records: Records = [
        (
            "P".to_owned(),
            record(&[("shared", &["mission.read", "mission.create"])], &[]),
        ),
        (
            "X".to_owned(),
            record(
                &[("local", &["mission.control"])],
                &[
                    ("group:a", "operation:X", "shared"),
                    ("group:b", "operation:X", "local"),
                ],
            ),
        ),
    ]
    .into();
    let access = compile_with_operations(&policy, &[], &records, &lineage).unwrap();
    assert!(access.orphaned().is_empty(), "{:?}", access.orphaned());
    let a = person("a1", &["a"]);
    assert!(is_allowed(&access, &a, "mission.create", "X", &lineage));
    let b = person("b1", &["b"]);
    assert!(is_allowed(&access, &b, "mission.control", "X", &lineage));
    assert!(!is_allowed(&access, &b, "mission.read", "X", &lineage));

    let mut shadowing = records.clone();
    shadowing
        .get_mut("X")
        .unwrap()
        .roles
        .insert("shared".into(), RoleDef::new(["mission.read"]));
    let access = compile_with_operations(&policy, &[], &shadowing, &lineage).unwrap();
    assert!(!is_allowed(&access, &a, "mission.create", "X", &lineage));
    assert!(is_allowed(&access, &a, "mission.read", "X", &lineage));
}

#[test]
fn a_role_edit_in_an_ancestor_is_checked_where_descendants_bind_it() {
    let lineage = tree();
    let mut policy = global();
    policy.delegable = Some(default_delegable());
    // X binds P's `shared` role; the editor holds mission.control at P but P's tier is edited
    // by someone holding it only at X's sibling.
    let records: Records = [
        (
            "P".to_owned(),
            record(&[("shared", &["mission.read"])], &[]),
        ),
        (
            "X".to_owned(),
            record(&[], &[("group:a", "operation:X", "shared")]),
        ),
    ]
    .into();
    let mut thin = policy.clone();
    thin.roles.insert(
        "lite".into(),
        RoleDef::new(["access.manage", "mission.read"]),
    );
    thin.rules
        .push(RuleDef::new("account:lite", "operation:P", "lite"));
    let after = record(&[("shared", &["mission.read", "mission.control"])], &[]);
    let refused = change(&thin, &records, &lineage, &person("lite", &[]), "P", &after)
        .err()
        .unwrap();
    assert!(refused.reason.contains("mission.control"), "{refused}");
    assert!(
        change(&thin, &records, &lineage, &person("ctl", &[]), "P", &after).is_ok(),
        "a Controller holds mission.control at P"
    );
}

/// A small xorshift generator, so the property test needs no dependency and replays.
struct Seeded(u64);

impl Seeded {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, count: usize) -> usize {
        usize::try_from(self.next() % count as u64).unwrap()
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}

#[test]
fn each_accepted_delegate_edit_only_allows_what_its_editor_held() {
    let lineage = tree();
    let mut policy = global();
    let mut cap = default_delegable();
    cap.push("access.manage".into());
    policy.delegable = Some(cap);
    let operations = ["ROOT", "P", "X", "S", "Q"];
    let people = [
        person("ctl", &[]),
        person("dep", &["deputies"]),
        person("u1", &["g1"]),
        person("u2", &["g2"]),
        person("u3", &["g1", "g2"]),
        person("nobody", &[]),
    ];
    let subjects = [
        "account:dep",
        "group:deputies",
        "group:g1",
        "group:g2",
        "account:u1",
        "account:ctl",
    ];
    let role_names = ["alpha", "beta", "gamma", "deputy", "operator"];
    let scopes: Vec<String> = operations
        .iter()
        .map(|id| format!("operation:{id}"))
        .collect();
    let grid = |access: &EffectiveAccess| -> BTreeSet<(String, String, String)> {
        let mut allowed = BTreeSet::new();
        for who in &people {
            for action in OPERATION_ACTIONS {
                for operation in operations {
                    if is_allowed(access, who, action, operation, &lineage) {
                        allowed.insert((
                            who.name.clone(),
                            (*action).to_owned(),
                            operation.to_string(),
                        ));
                    }
                }
            }
        }
        allowed
    };
    let mut generator = Seeded(0x9E37_79B9_7F4A_7C15);
    let mut accepted = 0;
    for _ in 0..500 {
        // A deputy of P who holds access.manage at P alone, not on the Operations under it.
        let mut deputy = record(
            &[("deputy", &["access.manage", "mission.read"])],
            &[("account:dep", "operation:P", "deputy")],
        );
        deputy.rules[0].propagate = false;
        let mut records = Records::from([("P".to_owned(), deputy)]);
        let mut access = compile_with_operations(&policy, &[], &records, &lineage).unwrap();
        for _ in 0..8 {
            let editor = match generator.below(10) {
                0..=4 => people[0].clone(),
                5..=7 => people[1].clone(),
                _ => generator.pick(&people).clone(),
            };
            let operation = match generator.below(10) {
                0..=3 => "P",
                4..=7 => "X",
                _ => *generator.pick(&operations),
            };
            let nearby: Vec<String> = lineage
                .subtree(operation)
                .into_iter()
                .map(|id| format!("operation:{id}"))
                .collect();
            let mut after = records.get(operation).cloned().unwrap_or_default();
            match generator.below(3) {
                0 => {
                    let name = (*generator.pick(&role_names[..3])).to_owned();
                    let count = 1 + generator.below(3);
                    let actions: BTreeSet<String> = (0..count)
                        .map(|_| (*generator.pick(OPERATION_ACTIONS)).to_owned())
                        .collect();
                    after
                        .roles
                        .insert(name, RoleDef::new(actions.into_iter().collect::<Vec<_>>()));
                }
                1 => {
                    let mut rule = RuleDef::new(
                        *generator.pick(&subjects),
                        if generator.below(10) == 0 {
                            generator.pick(&scopes).clone()
                        } else {
                            generator.pick(&nearby).clone()
                        },
                        *generator.pick(&role_names),
                    );
                    rule.propagate = generator.below(3) != 0;
                    let at = generator.below(after.rules.len() + 1);
                    after.rules.insert(at, rule);
                }
                _ => {
                    let at = generator.below(after.rules.len() + 1);
                    let mut rule = RuleDef::new(
                        *generator.pick(&subjects),
                        generator.pick(&nearby).clone(),
                        *generator.pick(&role_names),
                    );
                    rule.propagate = generator.below(3) != 0;
                    after.rules.insert(at, rule);
                }
            }
            let before = records.get(operation).cloned().unwrap_or_default();
            let set = AccessSet {
                global: &policy,
                operations: &records,
                lineage: &lineage,
            };
            let who = Editor::new(editor.clone(), &access);
            if check_operation_change(operation, &before, &after, &who, &access, &set).is_err() {
                continue;
            }
            accepted += 1;
            let was = grid(&access);
            let holder = access.clone();
            records.insert(operation.to_owned(), after);
            access = compile_with_operations(&policy, &[], &records, &lineage).unwrap();
            for (who, action, at) in grid(&access).difference(&was) {
                assert!(
                    holder.holds(&editor, action, &format!("operation:{at}"), &lineage),
                    "{who} may now {action} at {at}, which {} did not hold: {records:#?}",
                    editor.name
                );
            }
        }
    }
    assert!(
        accepted > 300,
        "the generator exercised {accepted} accepted edits"
    );
}
