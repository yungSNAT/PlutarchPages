//! The fork's tests of popup menus and host prompts.

use super::*;

fn value(ctx: &mut Context, script: &str) -> JsValue {
    ctx.eval(crate::Source::from_bytes(script)).unwrap_or_default()
}

#[test]
fn a_single_submenu_object_is_one_child() {
    let mut ctx = Context::default();
    let single = value(&mut ctx, "({cName:'parent', oSubMenu:{cName:'child'}})");
    let parsed = read_menu(&single, &mut ctx, 0, &mut 1024);
    let children: Vec<String> = parsed.map(|m| m.children.into_iter().map(|c| c.name).collect()).unwrap_or_default();
    assert_eq!(children, ["child"]);
}

#[test]
fn bad_deep_or_wide_submenus_are_refused() {
    let mut ctx = Context::default();
    let invalid = value(&mut ctx, "({cName:'parent', oSubMenu:42})");
    assert!(read_menu(&invalid, &mut ctx, 0, &mut 1024).is_err_and(|e| e.to_string().contains("invalid popup submenu")));

    let mut deep = String::from("{cName:'leaf'}");
    for _ in 0..18 {
        deep = format!("{{cName:'n', oSubMenu:{deep}}}");
    }
    let deep = value(&mut ctx, &format!("({deep})"));
    assert!(read_menu(&deep, &mut ctx, 0, &mut 1024).is_err_and(|e| e.to_string().contains("size or depth limit")));

    let wide = value(&mut ctx, "({cName:'p', oSubMenu: new Array(10).fill({cName:'c'})})");
    assert!(read_menu(&wide, &mut ctx, 0, &mut 5).is_err_and(|e| e.to_string().contains("too many entries")));
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn an_oversized_host_reply_is_refused() {
    let (tx, rx) = mpsc::channel();
    set_host(Some(tx));
    let responder = std::thread::spawn(move || {
        if let Ok(InteractiveEvent::Prompt(_, reply)) = rx.recv() {
            let _ = reply.send(Some("x".repeat(4097)));
        }
    });
    let result = ask(Prompt::Alert(crate::Message::plain("hi")));
    set_host(None);
    let _ = responder.join();
    assert!(result.is_err_and(|e| e.to_string().contains("exceeds its limit")));
}

#[test]
fn a_submenu_array_is_read_in_order() {
    let mut ctx = Context::default();
    let menu = value(&mut ctx, "({cName:'p', oSubMenu:[{cName:'a'}, {cName:'b'}]})");
    let names: Vec<String> = read_menu(&menu, &mut ctx, 0, &mut 1024).map(|m| m.children.into_iter().map(|c| c.name).collect()).unwrap_or_default();
    assert_eq!(names, ["a", "b"]);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn a_menu_answer_must_be_one_of_its_items_and_no_answer_is_null() {
    let mut session = crate::session::ScriptSession::default();
    let script = "String(app.popUpMenuEx({cName:'A', cReturn:'a'}));";
    let picked = session.run(crate::fork_test_support::job(script), &[Some("a".into())]);
    assert_eq!(picked.result.as_deref(), Some("a"));
    let invalid = session.run(crate::fork_test_support::job(script), &[Some("zzz".into())]);
    assert!(invalid.error.as_deref().is_some_and(|e| e.contains("invalid popup menu selection")), "{:?}", invalid.error);
    let none = session.run(crate::fork_test_support::job(script), &[None]);
    assert_eq!(none.result.as_deref(), Some("null"));
}
