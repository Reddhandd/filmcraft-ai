use crate::Session;
use serde_json::json;
fn demo() -> Session {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s
}

fn first_v1_clip(s: &Session) -> u64 {
    s.active_sequence().unwrap().video_tracks[0].items[0].id.0
}

#[test]
fn controller_static_video_parameter_is_guarded_and_undoable() {
    let mut s = demo();
    let clip = first_v1_clip(&s);
    s.execute("effects.apply", json!({"clips":[clip],"effect":"gaussian_blur"})).unwrap();
    let sequence_id = s.state.active_sequence.unwrap().0;
    let revision = s.revision;
    let before = s.execute("sequence.inspect", json!({})).unwrap();
    let index = before["video"][0]["items"][0]["effects"].as_array().unwrap().iter().position(|effect| effect["effect"] == "gaussian_blur").unwrap();
    let args = json!({"clip":clip,"effect":index,"effectId":"gaussian_blur",
        "param":"blurriness","value":25.0,"controllerStaticVideoParam":true,
        "expectedRevision":revision,"sequenceId":sequence_id});
    let mut wrong = args.clone();
    wrong["expectedRevision"] = json!(revision.saturating_sub(1));
    assert!(s.execute("effects.setParam", wrong).is_err());
    wrong = args.clone();
    wrong["sequenceId"] = json!(sequence_id + 1);
    assert!(s.execute("effects.setParam", wrong).is_err());
    wrong = args.clone();
    wrong["value"] = json!(3001.0);
    assert!(s.execute("effects.setParam", wrong).is_err());
    wrong = args.clone();
    wrong["effectId"] = json!("sharpen");
    assert!(s.execute("effects.setParam", wrong).is_err());
    assert_eq!(s.execute("sequence.inspect", json!({})).unwrap(), before);
    let applied = s.execute("effects.setParam", args).unwrap();
    assert_eq!(applied["effect"]["params"]["blurriness"]["value"], "Float(25.0)");
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.execute("sequence.inspect", json!({})).unwrap()["video"], before["video"]);
}

#[test]
fn controller_static_switch_and_choice_are_typed_and_undoable() {
    let mut s = demo();
    let clip = first_v1_clip(&s);
    s.execute("effects.apply", json!({"clips":[clip],"effect":"gaussian_blur"})).unwrap();
    let sequence_id = s.state.active_sequence.unwrap().0;
    let before = s.execute("sequence.inspect", json!({})).unwrap();
    let index = before["video"][0]["items"][0]["effects"].as_array().unwrap().iter().position(|effect| effect["effect"] == "gaussian_blur").unwrap();
    let mut args = json!({"clip":clip,"effect":index,"effectId":"gaussian_blur",
        "param":"repeat_edge","value":true,"controllerStaticVideoParam":true,
        "expectedRevision":s.revision,"sequenceId":sequence_id});
    let mut invalid = args.clone();
    invalid["value"] = json!(1);
    assert!(s.execute("effects.setParam", invalid).is_err());
    let switched = s.execute("effects.setParam", args.clone()).unwrap();
    assert_eq!(switched["effect"]["params"]["repeat_edge"]["value"], "Bool(true)");
    args["param"] = json!("dimensions");
    args["value"] = json!(1);
    args["expectedRevision"] = json!(s.revision);
    invalid = args.clone();
    invalid["value"] = json!(3);
    assert!(s.execute("effects.setParam", invalid).is_err());
    let chosen = s.execute("effects.setParam", args).unwrap();
    assert_eq!(chosen["effect"]["params"]["dimensions"]["value"], "Choice(1)");
    s.execute("edit.undo", json!({})).unwrap();
    let after_undo = s.execute("sequence.inspect", json!({})).unwrap();
    assert_eq!(after_undo["video"][0]["items"][0]["effects"][index]["params"]["dimensions"]["value"], "Choice(0)");
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.execute("sequence.inspect", json!({})).unwrap()["video"], before["video"]);
}

#[test]
fn controller_first_numeric_keyframe_is_guarded_and_undoable() {
    let mut s = demo();
    let clip = first_v1_clip(&s);
    s.execute("effects.apply", json!({"clips":[clip],"effect":"gaussian_blur"})).unwrap();
    let sequence_id = s.state.active_sequence.unwrap().0;
    let before = s.execute("sequence.inspect", json!({})).unwrap();
    let item = &before["video"][0]["items"][0];
    let index = item["effects"].as_array().unwrap().iter().position(|effect| effect["effect"] == "gaussian_blur").unwrap();
    let media_time = item["sourceIn"].as_i64().unwrap();
    let mut args = json!({"clip":clip,"effect":index,"effectId":"gaussian_blur",
        "param":"blurriness","value":50.0,"controllerVideoKeyframe":true,
        "clipTime":0,"expectedMediaTime":media_time,
        "expectedRevision":s.revision,"sequenceId":sequence_id});
    for (field, value) in [
        ("clipTime", json!(-1)),
        ("clipTime", item["duration"].clone()),
        ("expectedMediaTime", json!(media_time.saturating_add(1))),
        ("value", json!(3001.0)),
        ("param", json!("repeat_edge")),
        ("sequenceId", json!(sequence_id + 1)),
        ("expectedRevision", json!(s.revision.saturating_sub(1))),
    ] {
        let mut invalid = args.clone();
        invalid[field] = value;
        assert!(s.execute("effects.setParam", invalid).is_err(), "accepted {field}");
    }
    assert_eq!(s.execute("sequence.inspect", json!({})).unwrap(), before);
    let applied = s.execute("effects.setParam", args.clone()).unwrap();
    let param = &applied["effect"]["params"]["blurriness"];
    assert_eq!(param["keyframes"], 1);
    assert_eq!(param["keyframeDetails"][0]["time"], media_time);
    assert_eq!(param["keyframeDetails"][0]["value"], "Float(50.0)");
    args["expectedRevision"] = json!(s.revision);
    assert!(s.execute("effects.setParam", args).is_err());
    let after = s.execute("sequence.inspect", json!({})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.execute("sequence.inspect", json!({})).unwrap()["video"], before["video"]);
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(s.execute("sequence.inspect", json!({})).unwrap()["video"], after["video"]);
    let second_time = 254_016_000_i64;
    let second_media_time = media_time.saturating_add(second_time);
    let second = json!({"clip":clip,"effect":index,"effectId":"gaussian_blur",
        "param":"blurriness","value":100.0,"controllerVideoKeyframe":true,
        "clipTime":second_time,"expectedMediaTime":second_media_time,
        "expectedRevision":s.revision,"sequenceId":sequence_id});
    let mut duplicate = second.clone();
    duplicate["clipTime"] = json!(0);
    duplicate["expectedMediaTime"] = json!(media_time);
    assert!(s.execute("effects.setParam", duplicate).is_err());
    let applied = s.execute("effects.setParam", second).unwrap();
    let param = &applied["effect"]["params"]["blurriness"];
    assert_eq!(param["keyframes"], 2);
    assert_eq!(param["keyframeDetails"][0]["value"], "Float(50.0)");
    assert_eq!(param["keyframeDetails"][1]["time"], second_media_time);
    assert_eq!(param["keyframeDetails"][1]["value"], "Float(100.0)");
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.execute("sequence.inspect", json!({})).unwrap()["video"], after["video"]);
    s.execute("edit.redo", json!({})).unwrap();
    let before_remove = s.execute("sequence.inspect", json!({})).unwrap();
    let remove = json!({"clip":clip,"effect":index,"effectId":"gaussian_blur",
        "param":"blurriness","controllerRemoveVideoKeyframe":true,
        "clipTime":second_time,"expectedMediaTime":second_media_time,
        "expectedKeyframeValue":"Float(100.0)",
        "expectedRevision":s.revision,"sequenceId":sequence_id});
    let mut invalid = remove.clone();
    invalid["expectedKeyframeValue"] = json!("Float(99.0)");
    assert!(s.execute("effects.setParam", invalid).is_err());
    invalid = remove.clone();
    invalid["value"] = json!(0.0);
    assert!(s.execute("effects.setParam", invalid).is_err());
    assert_eq!(s.execute("sequence.inspect", json!({})).unwrap(), before_remove);
    let removed = s.execute("effects.setParam", remove.clone()).unwrap();
    assert_eq!(removed["effect"]["params"]["blurriness"]["keyframes"], 1);
    assert_eq!(removed["effect"]["params"]["blurriness"]["keyframeDetails"][0]["value"], "Float(50.0)");
    let after_remove = s.execute("sequence.inspect", json!({})).unwrap();
    let mut last_point = remove;
    last_point["clipTime"] = json!(0);
    last_point["expectedMediaTime"] = json!(media_time);
    last_point["expectedKeyframeValue"] = json!("Float(50.0)");
    last_point["expectedRevision"] = json!(s.revision);
    assert!(s.execute("effects.setParam", last_point).is_err());
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.execute("sequence.inspect", json!({})).unwrap()["video"], before_remove["video"]);
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(s.execute("sequence.inspect", json!({})).unwrap()["video"], after_remove["video"]);
}

#[test]
fn controller_existing_numeric_keyframe_changes_only_its_value() {
    let mut s = demo();
    let clip = first_v1_clip(&s);
    s.execute("effects.apply", json!({"clips":[clip],"effect":"gaussian_blur"})).unwrap();
    let sequence_id = s.state.active_sequence.unwrap().0;
    let inspected = s.execute("sequence.inspect", json!({})).unwrap();
    let item = &inspected["video"][0]["items"][0];
    let index = item["effects"].as_array().unwrap().iter().position(|effect| effect["effect"] == "gaussian_blur").unwrap();
    let media_time = item["sourceIn"].as_i64().unwrap();
    s.execute(
        "effects.setParam",
        json!({"clip":clip,"effect":index,"effectId":"gaussian_blur",
        "param":"blurriness","value":50.0,"controllerVideoKeyframe":true,
        "clipTime":0,"expectedMediaTime":media_time,
        "expectedRevision":s.revision,"sequenceId":sequence_id}),
    )
    .unwrap();
    let before = s.execute("sequence.inspect", json!({})).unwrap();
    let args = json!({"clip":clip,"effect":index,"effectId":"gaussian_blur",
        "param":"blurriness","value":75.0,"controllerExistingVideoKeyframe":true,
        "clipTime":0,"expectedMediaTime":media_time,"expectedKeyframeValue":"Float(50.0)",
        "expectedRevision":s.revision,"sequenceId":sequence_id});
    for (field, value) in [
        ("clipTime", json!(1)),
        ("expectedMediaTime", json!(media_time.saturating_add(1))),
        ("expectedKeyframeValue", json!("Float(49.0)")),
        ("value", json!(3001.0)),
        ("sequenceId", json!(sequence_id + 1)),
        ("expectedRevision", json!(s.revision.saturating_sub(1))),
    ] {
        let mut invalid = args.clone();
        invalid[field] = value;
        assert!(s.execute("effects.setParam", invalid).is_err(), "accepted {field}");
    }
    assert_eq!(s.execute("sequence.inspect", json!({})).unwrap(), before);
    let applied = s.execute("effects.setParam", args).unwrap();
    let prior = &before["video"][0]["items"][0]["effects"][index]["params"]["blurriness"];
    let changed = &applied["effect"]["params"]["blurriness"];
    assert_eq!(changed["value"], prior["value"]);
    assert_eq!(changed["keyframes"], prior["keyframes"]);
    assert_eq!(changed["keyframeDetails"][0]["time"], prior["keyframeDetails"][0]["time"]);
    assert_eq!(changed["keyframeDetails"][0]["interpolation"], prior["keyframeDetails"][0]["interpolation"]);
    assert_eq!(changed["keyframeDetails"][0]["value"], "Float(75.0)");
    let after = s.execute("sequence.inspect", json!({})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.execute("sequence.inspect", json!({})).unwrap()["video"], before["video"]);
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(s.execute("sequence.inspect", json!({})).unwrap()["video"], after["video"]);
}

#[test]
fn controller_keyframe_interpolation_changes_only_one_mode() {
    let mut s = demo();
    let clip = first_v1_clip(&s);
    s.execute("effects.apply", json!({"clips":[clip],"effect":"gaussian_blur"})).unwrap();
    let sequence_id = s.state.active_sequence.unwrap().0;
    let inspected = s.execute("sequence.inspect", json!({})).unwrap();
    let item = &inspected["video"][0]["items"][0];
    let index = item["effects"].as_array().unwrap().iter().position(|effect| effect["effect"] == "gaussian_blur").unwrap();
    let media_time = item["sourceIn"].as_i64().unwrap();
    s.execute(
        "effects.setParam",
        json!({"clip":clip,"effect":index,"effectId":"gaussian_blur",
        "param":"blurriness","value":50.0,"controllerVideoKeyframe":true,
        "clipTime":0,"expectedMediaTime":media_time,
        "expectedRevision":s.revision,"sequenceId":sequence_id}),
    )
    .unwrap();
    let before = s.execute("sequence.inspect", json!({})).unwrap();
    let args = json!({"clip":clip,"effect":index,"effectId":"gaussian_blur",
        "param":"blurriness","controllerKeyframeInterpolation":true,
        "clipTime":0,"expectedMediaTime":media_time,
        "expectedKeyframeValue":"Float(50.0)","expectedInterpolation":"Linear",
        "interpolation":"Hold","expectedRevision":s.revision,"sequenceId":sequence_id});
    for (field, value) in [
        ("clipTime", json!(1)),
        ("expectedMediaTime", json!(media_time.saturating_add(1))),
        ("expectedKeyframeValue", json!("Float(49.0)")),
        ("expectedInterpolation", json!("Bezier")),
        ("interpolation", json!("Unknown")),
        ("sequenceId", json!(sequence_id + 1)),
        ("expectedRevision", json!(s.revision.saturating_sub(1))),
        ("value", json!(50.0)),
    ] {
        let mut invalid = args.clone();
        invalid[field] = value;
        assert!(s.execute("effects.setParam", invalid).is_err(), "accepted {field}");
    }
    assert_eq!(s.execute("sequence.inspect", json!({})).unwrap(), before);
    let applied = s.execute("effects.setParam", args).unwrap();
    let changed = &applied["effect"]["params"]["blurriness"];
    let prior = &before["video"][0]["items"][0]["effects"][index]["params"]["blurriness"];
    assert_eq!(changed["value"], prior["value"]);
    assert_eq!(changed["keyframes"], prior["keyframes"]);
    assert_eq!(changed["keyframeDetails"][0]["time"], prior["keyframeDetails"][0]["time"]);
    assert_eq!(changed["keyframeDetails"][0]["value"], prior["keyframeDetails"][0]["value"]);
    assert_eq!(changed["keyframeDetails"][0]["interpolation"], "Hold");
    let after = s.execute("sequence.inspect", json!({})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.execute("sequence.inspect", json!({})).unwrap()["video"], before["video"]);
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(s.execute("sequence.inspect", json!({})).unwrap()["video"], after["video"]);
}

#[test]
fn controller_keyframe_move_preserves_value_and_interpolation() {
    let mut s = demo();
    let clip = first_v1_clip(&s);
    s.execute("effects.apply", json!({"clips":[clip],"effect":"gaussian_blur"})).unwrap();
    let sequence_id = s.state.active_sequence.unwrap().0;
    let inspected = s.execute("sequence.inspect", json!({})).unwrap();
    let item = &inspected["video"][0]["items"][0];
    let index = item["effects"].as_array().unwrap().iter().position(|effect| effect["effect"] == "gaussian_blur").unwrap();
    let media_time = item["sourceIn"].as_i64().unwrap();
    let second = 254_016_000_i64;
    let destination = second * 2;
    for (at, value) in [(0, 50.0), (second, 100.0)] {
        s.execute(
            "effects.setParam",
            json!({"clip":clip,"effect":index,"effectId":"gaussian_blur",
            "param":"blurriness","value":value,"controllerVideoKeyframe":true,
            "clipTime":at,"expectedMediaTime":media_time + at,
            "expectedRevision":s.revision,"sequenceId":sequence_id}),
        )
        .unwrap();
    }
    s.execute(
        "effects.setParam",
        json!({"clip":clip,"effect":index,"effectId":"gaussian_blur",
        "param":"blurriness","controllerKeyframeInterpolation":true,
        "clipTime":second,"expectedMediaTime":media_time + second,
        "expectedKeyframeValue":"Float(100.0)","expectedInterpolation":"Linear",
        "interpolation":"Hold","expectedRevision":s.revision,"sequenceId":sequence_id}),
    )
    .unwrap();
    let before = s.execute("sequence.inspect", json!({})).unwrap();
    let args = json!({"clip":clip,"effect":index,"effectId":"gaussian_blur",
        "param":"blurriness","controllerMoveVideoKeyframe":true,
        "clipTime":second,"expectedMediaTime":media_time + second,
        "toClipTime":destination,"expectedToMediaTime":media_time + destination,
        "expectedKeyframeValue":"Float(100.0)","expectedInterpolation":"Hold",
        "expectedRevision":s.revision,"sequenceId":sequence_id});
    for (field, value) in [
        ("clipTime", json!(0)),
        ("expectedMediaTime", json!(media_time)),
        ("toClipTime", json!(0)),
        ("expectedToMediaTime", json!(media_time + destination + 1)),
        ("expectedKeyframeValue", json!("Float(99.0)")),
        ("expectedInterpolation", json!("Linear")),
        ("sequenceId", json!(sequence_id + 1)),
        ("expectedRevision", json!(s.revision.saturating_sub(1))),
        ("value", json!(100.0)),
    ] {
        let mut invalid = args.clone();
        invalid[field] = value;
        assert!(s.execute("effects.setParam", invalid).is_err(), "accepted {field}");
    }
    assert_eq!(s.execute("sequence.inspect", json!({})).unwrap(), before);
    let applied = s.execute("effects.setParam", args).unwrap();
    let prior = &before["video"][0]["items"][0]["effects"][index]["params"]["blurriness"];
    let changed = &applied["effect"]["params"]["blurriness"];
    assert_eq!(changed["value"], prior["value"]);
    assert_eq!(changed["keyframes"], prior["keyframes"]);
    assert_eq!(changed["keyframeDetails"][0], prior["keyframeDetails"][0]);
    assert_eq!(changed["keyframeDetails"][1]["time"], media_time + destination);
    assert_eq!(changed["keyframeDetails"][1]["value"], "Float(100.0)");
    assert_eq!(changed["keyframeDetails"][1]["interpolation"], "Hold");
    let after = s.execute("sequence.inspect", json!({})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.execute("sequence.inspect", json!({})).unwrap()["video"], before["video"]);
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(s.execute("sequence.inspect", json!({})).unwrap()["video"], after["video"]);
}

#[test]
fn controller_video_effect_removal_is_guarded_and_undoable() {
    let mut s = demo();
    let clip = first_v1_clip(&s);
    s.execute("effects.apply", json!({"clips":[clip],"effect":"gaussian_blur"})).unwrap();
    let sequence_id = s.state.active_sequence.unwrap().0;
    let revision = s.revision;
    let before = s.execute("sequence.inspect", json!({})).unwrap();
    let index = before["video"][0]["items"][0]["effects"].as_array().unwrap().iter().position(|effect| effect["effect"] == "gaussian_blur").unwrap();
    let args = json!({"clip":clip,"index":index,"effectId":"gaussian_blur",
        "controllerSingleVideoEffect":true,"expectedRevision":revision,"sequenceId":sequence_id});
    let mut wrong = args.clone();
    wrong["expectedRevision"] = json!(revision.saturating_sub(1));
    assert!(s.execute("effects.remove", wrong).is_err());
    wrong = args.clone();
    wrong["sequenceId"] = json!(sequence_id + 1);
    assert!(s.execute("effects.remove", wrong).is_err());
    wrong = args.clone();
    wrong["effectId"] = json!("sharpen");
    assert!(s.execute("effects.remove", wrong).is_err());
    assert_eq!(s.execute("sequence.inspect", json!({})).unwrap(), before);
    let removed = s.execute("effects.remove", args).unwrap();
    assert_eq!(removed["removed"]["effect"], "gaussian_blur");
    assert_eq!(removed["removed"], before["video"][0]["items"][0]["effects"][index]);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.execute("sequence.inspect", json!({})).unwrap()["video"], before["video"]);
}

#[test]
fn hostile_revision_guards_reject_before_any_project_or_history_change() {
    let mut s = demo();
    let original = s.project.clone();
    let history = s.history.undo.len();
    for params in [
        json!({"expectedRevision":"1"}),
        json!({"expectedRevision":-1}),
        json!({"expectedRevision":s.revision+1}),
        json!({"sequenceId":0}),
        json!({"expectedRevision":s.revision,"sequenceId":s.state.active_sequence.unwrap().0,"controllerUnknown":true}),
    ] {
        assert!(s.execute("markers.add", params).is_err());
        assert_eq!(*s.project, *original);
        assert_eq!(s.history.undo.len(), history);
    }
}
