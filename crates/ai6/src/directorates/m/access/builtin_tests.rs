use super::*;

fn person(name: &str, groups: &[&str]) -> Person {
    Person {
        id: format!("person-{name}"),
        name: name.to_owned(),
        groups: groups.iter().map(|group| (*group).to_owned()).collect(),
        ..Person::default()
    }
}

fn ask(person: &Person, action: &str, resource: &str) -> AuthorizationRequest {
    AuthorizationRequest {
        person: person.clone(),
        action: action.to_owned(),
        resource: resource.to_owned(),
        ancestors: Vec::new(),
    }
}

fn allowed(access: &EffectiveAccess, person: &Person, action: &str, resource: &str) -> bool {
    access
        .authorize(ask(person, action, resource))
        .unwrap()
        .allowed
}

fn policy(toml: &str) -> AccessPolicy {
    AccessPolicy::from_toml(toml).unwrap()
}

fn seeded_with(rules: &[(&str, &str, &str)], chief: &[&str]) -> AccessPolicy {
    let mut policy = seeded_policy();
    for (subject, scope, role) in rules {
        policy.rules.push(RuleDef::new(*subject, *scope, *role));
    }
    policy.chief = chief.iter().map(|subject| (*subject).to_owned()).collect();
    policy
}

fn actions_of<'a>(policy: &'a AccessPolicy, role: &str) -> Vec<&'a str> {
    policy.roles[role]
        .actions
        .iter()
        .map(String::as_str)
        .collect()
}

fn editor(access: &EffectiveAccess, who: &Person) -> Editor {
    Editor::new(who.clone(), access)
}

#[test]
fn the_action_lists_are_sorted_and_the_operation_actions_are_actions() {
    for list in [ACTIONS, OPERATION_ACTIONS] {
        assert!(list.windows(2).all(|pair| pair[0] < pair[1]), "sorted");
    }
    for action in OPERATION_ACTIONS {
        assert!(ACTIONS.contains(action), "{action}");
    }
    for action in ["access.manage", "access.read", "account.manage"] {
        assert!(ACTIONS.contains(&action));
    }
    assert!(!OPERATION_ACTIONS.contains(&"account.manage"));
    assert!(!OPERATION_ACTIONS.contains(&"station.configure"));
}

#[test]
fn the_seeded_roles_hold_exactly_the_default_table() {
    let seeded = seeded_policy();
    let read = [
        "agent.read",
        "cable.read",
        "mission.read",
        "pool.read",
        "roster.read",
        "session.read",
        "station.read",
    ];
    let work = [
        "approval.decide",
        "cable.send",
        "mission.control",
        "mission.create",
        "session.input",
    ];
    let mut operator: Vec<&str> = [&read[..], &work[..]].concat();
    operator.sort_unstable();
    let mut auditor: Vec<&str> = [&read[..], &["access.read", "audit.read"][..]].concat();
    auditor.sort_unstable();
    let mut controller: Vec<&str> = [
        &operator[..],
        &[
            "access.manage",
            "access.read",
            "account.manage",
            "agent.configure",
            "approval.class",
            "audit.read",
            "pool.manage",
            "roster.manage",
            "station.configure",
        ][..],
    ]
    .concat();
    controller.sort_unstable();
    assert_eq!(
        seeded.roles.keys().map(String::as_str).collect::<Vec<_>>(),
        ["auditor", "controller", "operator"]
    );
    assert_eq!(actions_of(&seeded, "operator"), operator);
    assert_eq!(actions_of(&seeded, "auditor"), auditor);
    assert_eq!(actions_of(&seeded, "controller"), controller);
    assert!(seeded.rules.is_empty() && seeded.chief.is_empty() && seeded.delegable.is_none());
    for role in seeded.roles.values() {
        assert!(!role.actions.iter().any(|a| a == "assistant.memory.admin"));
    }
    compile(&seeded, &[]).unwrap();
    assert_eq!(
        AccessPolicy::from_json(&seeded.to_json()).unwrap(),
        seeded,
        "the record round-trips through its JSON form"
    );
}

#[test]
fn the_default_cap_withholds_the_actions_that_must_not_be_delegated() {
    let cap = default_delegable();
    for withheld in ["access.manage", "approval.class"] {
        assert!(!cap.iter().any(|action| action == withheld), "{withheld}");
    }
    assert_eq!(cap.len(), OPERATION_ACTIONS.len() - 2);
}

#[test]
fn the_chief_holds_an_action_added_after_its_rule_was_written() {
    let boss = person("boss", &[]);
    let stored = seeded_with(&[], &["account:boss"]);
    let mut actions = ACTIONS.to_vec();
    actions.push("future.thing");
    let later = effective::compile_with_actions(&stored, &[], &actions).unwrap();
    assert!(allowed(&later, &boss, "future.thing", "station:hq"));
    let now = compile(&stored, &[]).unwrap();
    assert!(!allowed(&now, &boss, "future.thing", "station:hq"));
    assert!(allowed(
        &now,
        &boss,
        "assistant.memory.admin",
        "config:assistant"
    ));
}

#[test]
fn a_global_rule_placed_first_does_not_shadow_a_chief() {
    let stored = seeded_with(&[("group:staff", "*", "auditor")], &["account:boss"]);
    let access = compile(&stored, &[]).unwrap();
    let boss = person("boss", &["staff"]);
    assert!(allowed(&access, &boss, "station.configure", "station:hq"));
    let clerk = person("clerk", &["staff"]);
    assert!(!allowed(&access, &clerk, "station.configure", "station:hq"));
    assert!(allowed(&access, &clerk, "mission.read", "station:hq"));
}

#[test]
fn the_host_chief_is_chief_whatever_is_stored() {
    let stored = seeded_with(&[], &[]);
    let access = compile(&stored, &["group:root".to_owned()]).unwrap();
    let root = person("root", &["root"]);
    assert!(access.is_chief(&root));
    assert!(allowed(&access, &root, "account.manage", "config:accounts"));
    assert_eq!(access.host_chiefs(), ["group:root"]);
}

#[test]
fn chief_is_built_in_and_cannot_be_stored_or_bound() {
    let role = policy("[role.chief]\nactions = ['mission.read']");
    let error = compile(&role, &[]).unwrap_err().to_string();
    assert!(error.contains("chief is built in"), "{error}");
    let mut rule = seeded_with(&[], &[]);
    rule.rules.push(RuleDef::new("account:x", "*", "chief"));
    let error = compile(&rule, &[]).unwrap_err().to_string();
    assert!(error.contains("chief is built in"), "{error}");
    let mut at = seeded_with(&[], &[]);
    at.roles
        .insert("a@operation".into(), RoleDef::new(["mission.read"]));
    assert!(
        compile(&at, &[]).is_err(),
        "'@' is reserved for compiled copies"
    );
    assert!(compile(&seeded_with(&[], &["nobody"]), &[]).is_err());
    let mut cap = seeded_with(&[], &[]);
    cap.delegable = Some(vec!["station.configure".into()]);
    assert!(compile(&cap, &[]).is_err());
}

#[test]
fn only_a_chief_may_change_the_list_and_none_may_leave_no_chief() {
    let boss = person("boss", &[]);
    let ctl = person("ctl", &[]);
    let before = seeded_with(&[("account:ctl", "*", "controller")], &["account:boss"]);
    let access = compile(&before, &[]).unwrap();
    let lineage = Lineage::default();

    let mut cleared = before.clone();
    cleared.chief.clear();
    let refused = check_change(
        &before,
        &cleared,
        &editor(&access, &boss),
        &access,
        &lineage,
    );
    assert!(refused.unwrap_err().reason.contains("no chief"));

    let mut other = before.clone();
    other.chief.push("account:ctl".into());
    let refused = check_change(&before, &other, &editor(&access, &ctl), &access, &lineage);
    assert!(refused.unwrap_err().reason.contains("only a chief"));
    check_change(&before, &other, &editor(&access, &boss), &access, &lineage).unwrap();

    let hosted = compile(&before, &["account:root".to_owned()]).unwrap();
    check_change(
        &before,
        &cleared,
        &editor(&hosted, &boss),
        &hosted,
        &lineage,
    )
    .expect("a host chief is a chief, so the stored list may go");
}

#[test]
fn a_controller_cannot_escalate_but_can_share_what_it_holds() {
    let boss = person("boss", &[]);
    let ctl = person("ctl", &[]);
    let before = seeded_with(&[("account:ctl", "*", "controller")], &["account:boss"]);
    let access = compile(&before, &[]).unwrap();
    let lineage = Lineage::default();
    let as_ctl = editor(&access, &ctl);

    let mut memory = before.clone();
    memory
        .roles
        .get_mut("controller")
        .unwrap()
        .actions
        .push("assistant.memory.admin".into());
    let refused = check_change(&before, &memory, &as_ctl, &access, &lineage).unwrap_err();
    assert!(
        refused.reason.contains("assistant.memory.admin"),
        "{refused}"
    );

    let mut chief = before.clone();
    chief.chief.clear();
    assert!(check_change(&before, &chief, &as_ctl, &access, &lineage).is_err());

    let mut shared = before.clone();
    shared
        .roles
        .get_mut("operator")
        .unwrap()
        .actions
        .push("pool.manage".into());
    check_change(&before, &shared, &as_ctl, &access, &lineage).unwrap();

    let mut cap = before.clone();
    cap.delegable = Some(vec!["mission.read".into(), "access.manage".into()]);
    check_change(&before, &cap, &as_ctl, &access, &lineage)
        .expect("a controller holds access.manage at *, so it may make it delegable");
    check_change(&before, &memory, &editor(&access, &boss), &access, &lineage).unwrap();

    let mut removal = before.clone();
    removal.roles.get_mut("operator").unwrap().actions.pop();
    check_change(&before, &removal, &as_ctl, &access, &lineage).expect("removals pass");

    let nobody = editor(&access, &person("nobody", &[]));
    assert!(
        check_change(&before, &removal, &nobody, &access, &lineage)
            .unwrap_err()
            .reason
            .contains("access.manage")
    );
}

#[test]
fn an_auditor_with_read_access_cannot_change_anything() {
    let aud = person("aud", &[]);
    let before = seeded_with(&[("account:aud", "*", "auditor")], &["account:boss"]);
    let access = compile(&before, &[]).unwrap();
    let mut after = before.clone();
    after.roles.get_mut("operator").unwrap().actions.pop();
    assert!(
        check_change(
            &before,
            &after,
            &editor(&access, &aud),
            &access,
            &Lineage::default()
        )
        .is_err()
    );
}

#[test]
fn a_role_bound_at_an_operation_grants_only_its_operation_actions_there() {
    let stored = policy(
        r#"
        [role.mixed]
        actions = ["mission.read", "station.configure"]
        [role.stationary]
        actions = ["station.configure"]
        [[rule]]
        subject = "account:ann"
        scope = "operation:G"
        role = "mixed"
        [[rule]]
        subject = "account:ann"
        scope = "station:hq"
        role = "mixed"
        [[rule]]
        subject = "account:bob"
        scope = "operation:G"
        role = "stationary"
        [[rule]]
        subject = "account:bob"
        scope = "operation:G"
        role = "mixed"
        "#,
    );
    let access = compile(&stored, &[]).unwrap();
    let ann = person("ann", &[]);
    assert!(allowed(&access, &ann, "mission.read", "operation:G"));
    assert!(!allowed(&access, &ann, "station.configure", "operation:G"));
    assert!(allowed(&access, &ann, "mission.read", "station:hq"));
    assert!(allowed(&access, &ann, "station.configure", "station:hq"));
    let bob = person("bob", &[]);
    assert!(
        !allowed(&access, &bob, "mission.read", "operation:G"),
        "a role with nothing to grant at an Operation still decides, first match"
    );
}

#[test]
fn explain_names_the_tier_the_rule_and_the_role() {
    let stored = seeded_with(
        &[
            ("group:qa", "operation:G", "auditor"),
            ("group:qa", "*", "operator"),
        ],
        &["account:boss"],
    );
    let access = compile(&stored, &["account:root".to_owned()]).unwrap();
    let qa = person("qa1", &["qa"]);
    let on_g = access
        .explain(&ask(&qa, "audit.read", "operation:G"))
        .unwrap();
    assert_eq!(
        (on_g.allowed, on_g.tier, on_g.rule_index, on_g.role),
        (
            true,
            Some(TierKind::Global),
            Some(0),
            Some("auditor".to_owned())
        )
    );
    let elsewhere = access
        .explain(&ask(&qa, "mission.create", "operation:H"))
        .unwrap();
    assert_eq!(elsewhere.rule_index, Some(1));
    assert_eq!(elsewhere.role.as_deref(), Some("operator"));
    let boss = access
        .explain(&ask(&person("boss", &[]), "pool.manage", "*"))
        .unwrap();
    assert_eq!(
        (boss.tier, boss.rule_index),
        (Some(TierKind::Chief), Some(0))
    );
    let root = access
        .explain(&ask(&person("root", &[]), "pool.manage", "*"))
        .unwrap();
    assert_eq!(
        (root.tier, root.rule_index),
        (Some(TierKind::Chief), Some(1))
    );
    let denied = access
        .explain(&ask(&qa, "station.configure", "station:hq"))
        .unwrap();
    assert!(
        !denied.allowed
            && denied.tier.is_none()
            && denied.rule_index.is_none()
            && denied.role.is_none()
    );
}

#[test]
fn explain_and_the_authorizer_agree_on_a_grid_of_requests() {
    let stored = seeded_with(
        &[
            ("group:a", "operation:P", "auditor"),
            ("account:z", "operation:P", "controller"),
            ("group:a", "*", "operator"),
            ("group:b", "station:hq", "controller"),
        ],
        &["account:boss"],
    );
    let access = compile(&stored, &[]).unwrap();
    let people = [
        person("x", &["a"]),
        person("z", &["a", "b"]),
        person("y", &["b"]),
        person("boss", &[]),
        person("w", &[]),
    ];
    for who in &people {
        for action in ACTIONS {
            for resource in [
                "*",
                "operation:P",
                "operation:Q",
                "station:hq",
                "config:access",
            ] {
                let mut request = ask(who, action, resource);
                if resource == "operation:Q" {
                    request.ancestors = vec!["operation:P".into()];
                }
                assert_eq!(
                    access.authorize(request.clone()).unwrap().allowed,
                    access.explain(&request).unwrap().allowed,
                    "{} {action} {resource}",
                    who.name
                );
            }
        }
    }
}

#[test]
fn a_policy_file_with_a_chief_and_a_cap_reads() {
    let access = access_from_toml(
        "chief = ['account:boss']\ndelegable = ['mission.read']\n[role.v]\nactions = ['mission.read']",
    )
    .unwrap();
    assert!(access.is_chief(&person("boss", &[])));
    assert!(access_from_toml("delegable = ['nothing.such']").is_err());
    assert!(access_from_toml("[role.v]\nactions = []").is_err());
}

#[test]
fn a_rule_that_reaches_down_needs_the_action_held_down_there_too() {
    let mut before = seeded_with(&[], &["account:boss"]);
    before
        .roles
        .insert("gm".into(), RoleDef::new(["access.manage"]));
    before.roles.insert(
        "lead".into(),
        RoleDef::new(["mission.control", "access.read"]),
    );
    before
        .rules
        .push(RuleDef::new("account:lite", "config:access", "gm"));
    let mut here = RuleDef::new("account:lite", "operation:P", "lead");
    here.propagate = false;
    before.rules.push(here);
    let access = compile(&before, &[]).unwrap();
    let lineage = Lineage::new([("P", vec![]), ("X", vec!["P"])]);
    let lite = editor(&access, &person("lite", &[]));

    let mut after = before.clone();
    after
        .rules
        .push(RuleDef::new("group:g", "operation:P", "lead"));
    let refused = check_change(&before, &after, &lite, &access, &lineage).unwrap_err();
    assert!(refused.reason.contains("operation:X"), "{refused}");

    let mut narrow = before.clone();
    let mut rule = RuleDef::new("group:g", "operation:P", "lead");
    rule.propagate = false;
    narrow.rules.push(rule);
    check_change(&before, &narrow, &lite, &access, &lineage)
        .expect("held at P, and the rule stays at P");
}
