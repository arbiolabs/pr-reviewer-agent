mod common;

use common::*;
use pr_review_tui::model::{Model, SaveState, Screen};
use pr_review_tui::protocol::{ClientMessage, Command, RequestId, ServerMessage, Text};
use pr_review_tui::update::{Effect, Msg, NetEvent, update};

fn send(m: &mut Model, json: &str) -> Vec<Effect> {
    update(m, Msg::Net(NetEvent::Message(ServerMessage::parse(json).unwrap())))
}

/// The desktop on Settings, live on an engine daemon that grants settings.set.
fn desktop() -> Model {
    let mut m = model();
    m.agent.wanted = true;
    m.ui.screen = Screen::Settings;
    update(&mut m, Msg::Net(NetEvent::Opened));
    send(
        &mut m,
        &read("fixtures/server.welcome.engine.json")
            .replace("\"review.publish\"", "\"review.publish\", \"settings.set\""),
    );
    send(&mut m, &read("fixtures/server.snapshot.json"));
    m
}

fn settings_reply(id: &RequestId) -> String {
    read("fixtures/server.settings.json").replace("req-settings", id.as_str())
}

#[test]
fn settings_open_asks_once_and_keeps_the_answer() {
    let mut m = desktop();
    let asked = m.agent.request.clone().expect("Settings asks for the model on its own");
    send(&mut m, &settings_reply(&asked));
    let s = m.agent.settings.as_ref().unwrap();
    assert_eq!((s.model.reference.as_str(), s.model.source.as_str()), ("anthropic/claude-opus-5-5", "config"));
    assert_eq!(s.models.len(), 4);
    assert_eq!(update(&mut m, Msg::Tick(10)), []);
}

#[test]
fn the_tui_never_asks() {
    let mut m = desktop();
    m.agent.wanted = false;
    m.agent.request = None;
    assert_eq!(update(&mut m, Msg::Tick(10)), []);
}

#[test]
fn save_sends_settings_set_then_asks_again_once_accepted() {
    let mut m = desktop();
    let asked = m.agent.request.clone().unwrap();
    send(&mut m, &settings_reply(&asked));
    let fx = update(&mut m, Msg::SetModel("openai/gpt-5.5".into()));
    let [Effect::Send(ClientMessage::Command(c))] = fx.as_slice() else { panic!("{fx:?}") };
    assert_eq!(c.idempotency_key, "settings.set:openai/gpt-5.5");
    assert!(matches!(&c.command, Command::SetSettings(p) if p.model.as_deref() == Some("openai/gpt-5.5")));
    let id = c.request_id.clone();
    let accepted = format!(
        r#"{{"type":"command.result","requestId":"{id}","status":"accepted","operationId":"settings.set:openai/gpt-5.5"}}"#
    );
    let fx = send(&mut m, &accepted);
    assert_eq!(m.agent.save, SaveState::Saved("openai/gpt-5.5".into()));
    assert!(matches!(fx.as_slice(), [Effect::Send(ClientMessage::SettingsGet { .. })]), "{fx:?}");
    assert!(m.agent.settings.is_some(), "the last answer stays until the new one");
}

#[test]
fn the_review_message_saves_and_resets_under_its_own_state_and_the_keys_the_daemon_expects() {
    let mut m = desktop();
    let asked = m.agent.request.clone().unwrap();
    send(&mut m, &settings_reply(&asked));
    let t = m.agent.settings.as_ref().unwrap().summary_template.as_ref().expect("the daemon sends the template");
    assert!(t.is_default && t.value == t.default && t.variables.iter().any(|v| v.name.as_str() == "findings_list"));

    let fixture: serde_json::Value =
        serde_json::from_str(&read("fixtures/client.command.settings.set.template.json")).unwrap();
    let custom = fixture["params"]["summaryTemplate"].as_str().unwrap().to_string();
    let fx = update(&mut m, Msg::SetSummaryTemplate(custom.clone()));
    let [Effect::Send(ClientMessage::Command(c))] = fx.as_slice() else { panic!("{fx:?}") };
    assert_eq!(c.idempotency_key, fixture["idempotencyKey"].as_str().unwrap(), "the same hash the daemon computes");
    assert_eq!(m.agent.save, SaveState::Idle, "the model's save is not the message's");
    let id = c.request_id.clone();
    let fx = send(
        &mut m,
        &format!(r#"{{"type":"command.result","requestId":"{id}","status":"accepted","operationId":"x"}}"#),
    );
    assert_eq!(m.agent.template_save, SaveState::Saved(custom));
    assert!(matches!(fx.as_slice(), [Effect::Send(ClientMessage::SettingsGet { .. })]), "{fx:?}");

    let fx = update(&mut m, Msg::SetSummaryTemplate(String::new()));
    let [Effect::Send(ClientMessage::Command(c))] = fx.as_slice() else { panic!("{fx:?}") };
    assert_eq!(c.idempotency_key, "settings.set::template:default");
    let refused = format!(
        r#"{{"type":"command.result","requestId":"{}","status":"rejected","code":"INVALID_REQUEST","message":"unknown variable {{{{nope}}}}"}}"#,
        c.request_id
    );
    send(&mut m, &refused);
    assert_eq!(m.agent.template_save, SaveState::Refused(Text::new("unknown variable {{nope}}")));
}

#[test]
fn a_refusal_and_a_read_only_daemon_show_the_daemons_reason() {
    let mut m = desktop();
    let asked = m.agent.request.clone().unwrap();
    send(&mut m, &settings_reply(&asked));
    let fx = update(&mut m, Msg::SetModel("openai/gpt-5.5".into()));
    let [Effect::Send(ClientMessage::Command(c))] = fx.as_slice() else { panic!("{fx:?}") };
    let refused = format!(
        r#"{{"type":"command.result","requestId":"{}","status":"rejected","code":"NOT_FOUND","message":"model openai/gpt-5.5 is not available"}}"#,
        c.request_id
    );
    assert_eq!(send(&mut m, &refused), []);
    assert_eq!(m.agent.save, SaveState::Refused(Text::new("model openai/gpt-5.5 is not available")));

    let mut ro = model();
    ro.agent.wanted = true;
    ro.ui.screen = Screen::Settings;
    update(&mut ro, Msg::Net(NetEvent::Opened));
    send(&mut ro, &read("fixtures/server.welcome.read-only.json"));
    let asked = ro.agent.request.clone().unwrap();
    send(
        &mut ro,
        &format!(
            r#"{{"type":"error","requestId":"{asked}","code":"CAPABILITY_DENIED","message":"settings.get is not available: commands are off"}}"#
        ),
    );
    assert_eq!(ro.agent.error, Some(Text::new("settings.get is not available: commands are off")));
    assert_eq!(ro.agent.request, None);
    update(&mut ro, Msg::SetModel("openai/gpt-5.5".into()));
    assert!(matches!(ro.agent.save, SaveState::Refused(_)));
}
