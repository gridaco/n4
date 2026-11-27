#![cfg(feature = "host-audio")]

use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    fs,
    io::{self, BufRead, BufReader, Write},
    path::Path,
    process::{Child, Command, Stdio},
    sync::mpsc::{Receiver, channel},
    thread,
    time::Duration,
};

struct Client {
    child: Child,
    output: Receiver<io::Result<String>>,
    next_id: u64,
}

impl Client {
    fn open(project: &Path) -> Result<Self> {
        let mut child = Command::new(env!("CARGO_BIN_EXE_n4"))
            .arg("editor")
            .arg(project)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?;
        let stdout = child.stdout.take().context("missing CLI output")?;
        let (send, output) = channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                if send.send(line).is_err() {
                    break;
                }
            }
        });
        Ok(Self {
            child,
            output,
            next_id: 0,
        })
    }

    fn next(&self) -> Result<Value> {
        let line = self
            .output
            .recv_timeout(Duration::from_secs(20))
            .context("waiting for an editor message")??;
        serde_json::from_str(&line).with_context(|| format!("invalid JSONL output: {line}"))
    }

    fn request(&mut self, mut command: Value) -> Result<Value> {
        self.next_id += 1;
        command["id"] = json!(self.next_id);
        let input = self.child.stdin.as_mut().context("missing CLI input")?;
        writeln!(input, "{command}")?;
        input.flush()?;
        loop {
            let response = self.next()?;
            if response.get("event").is_none() {
                assert_eq!(response["id"], self.next_id);
                return Ok(response);
            }
        }
    }

    fn run(&mut self, command: Value) -> Result<Value> {
        let reply = self.request(command)?;
        ensure!(reply.get("error").is_none(), "{reply}");
        Ok(reply["result"].clone())
    }

    fn state(&mut self) -> Result<Value> {
        self.run(json!({"op":"getState"}))
    }

    fn set_v2(&mut self, enabled: bool) -> Result<Value> {
        let state = self.state()?;
        self.run(json!({
            "op":"setTrackEnabled", "revision":state["revision"], "scope":"root",
            "track":"V2", "enabled":enabled
        }))
    }

    fn pixel(&mut self, frame: u32) -> Result<Value> {
        let state = self.state()?;
        let result = self.run(json!({
            "op":"render", "revision":state["revision"], "frame":frame, "points":[[10,8]]
        }))?;
        Ok(result["pixels"][0].clone())
    }

    fn close(mut self) -> Result<()> {
        drop(self.child.stdin.take());
        ensure!(self.child.wait()?.success(), "editor did not exit cleanly");
        Ok(())
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn editor_cli_picks_and_persists_root_track_visibility() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let master = directory.path().join("project.proj");
    let sequences = directory.path().join("sequences");
    let original = concat!(
        "project editor fps 24/1 size 64x36\r\n",
        "# Keep the review notes and repeated overlays.\r\n",
        "[0,4) V1 plate\r\n[0,4) V2 badge\r\n",
        "[4,8) V1 section\r\n  [0,4) V1 plate\r\n  [0,4) V2 nested\r\n",
        "[8,12) V1 plate\r\n[8,12) V2 badge\r\n",
    );
    fs::write(&master, original)?;
    fs::create_dir(&sequences)?;
    for (name, color) in [
        ("plate", "#ff0000"),
        ("badge", "#00ff00"),
        ("nested", "#0000ff"),
    ] {
        let background = if name == "plate" {
            r##" background="#000000""##
        } else {
            ""
        };
        fs::write(
            sequences.join(format!("{name}.xml")),
            format!(
                r#"<sequence{background}><rect x="4" y="4" width="24" height="16" fill="{color}"/></sequence>"#
            ),
        )?;
    }

    let mut client = Client::open(directory.path())?;
    let malformed = client.request(json!({"op":"unknown"}))?;
    assert_eq!(malformed["error"]["code"], "invalidRequest");
    let initial = client.state()?;
    let revision = &initial["revision"];
    let first =
        client.run(json!({"op":"hitTest", "revision":revision, "frame":2, "point":[10,8]}))?;
    let reprise =
        client.run(json!({"op":"hitTest", "revision":revision, "frame":10, "point":[10,8]}))?;
    assert_eq!(first["indeterminate"], false);
    assert_eq!(
        first["hits"][0],
        json!({"occurrence":"V2:badge@0","layers":[0]})
    );
    assert_eq!(
        first["hits"][1],
        json!({"occurrence":"V1:plate@0","layers":[0]})
    );
    assert_eq!(
        reprise["hits"][0],
        json!({"occurrence":"V2:badge@8","layers":[0]})
    );
    let selected = json!({"kind":"layer","occurrence":"V2:badge@0","layers":[0]});
    client.run(json!({"op":"seek", "frame":2}))?;
    let placement = client.run(json!({
        "op":"select", "revision":revision, "target":{"kind":"placement","occurrence":"V2:badge@0"}
    }))?;
    assert_eq!(placement["selectionSource"]["file"], "project.proj");
    let picked = client.run(json!({
        "op":"selectAt", "revision":revision, "frame":2, "point":[10,8]
    }))?;
    assert_eq!(picked["selection"], selected);
    assert_eq!(picked["selectionSource"]["file"], "sequences/badge.xml");
    client.run(json!({"op":"clearSelection"}))?;
    client.run(json!({"op":"select", "revision":revision, "target":selected}))?;
    let state = client.state()?;
    assert_eq!(state["frame"], 2);
    assert_eq!(state["selection"], selected);
    assert_eq!(state["revision"], *revision);
    assert_eq!(state["canUndo"], false);
    let geometry = client.run(json!({
        "op":"layerGeometry", "revision":revision, "frame":2, "layer":first["hits"][0]
    }))?;
    assert_eq!(geometry["target"], first["hits"][0]);
    assert!(geometry["compositionQuad"].is_array());

    let hover = client.run(json!({"op":"hoverAt","revision":revision,"frame":2,"point":[10,8]}))?;
    assert!(hover["hover"].is_null());
    client.run(json!({"op":"clearSelection"}))?;
    let hover = client.run(json!({"op":"hoverAt","revision":revision,"frame":2,"point":[10,8]}))?;
    assert_eq!(hover["hover"], first["hits"][0]);
    assert_eq!(hover["selections"], json!([]));
    let began = client.run(json!({
        "op":"beginMarquee","revision":revision,"frame":2,"point":[30,24]
    }))?;
    assert!(began["marquee"].is_null());
    assert!(began["hover"].is_null());
    let moved = client.run(json!({"op":"updateMarquee","revision":revision,"point":[-10,-5]}))?;
    assert_eq!(moved["marquee"], json!([0.0, 0.0, 30.0, 24.0]));
    assert_eq!(moved["selections"], json!([]));
    let finished =
        client.run(json!({"op":"finishMarquee","revision":revision,"point":[-10,-5]}))?;
    assert!(finished["marquee"].is_null());
    assert_eq!(
        finished["selections"],
        json!([selected, {"kind":"layer","occurrence":"V1:plate@0","layers":[0]}])
    );
    assert_eq!(finished["selection"], selected);
    assert_eq!(finished["revision"], *revision);
    assert_eq!(finished["canUndo"], false);
    client.run(json!({"op":"beginMarquee","revision":revision,"frame":2,"point":[0,0]}))?;
    client.run(json!({"op":"updateMarquee","revision":revision,"point":[30,24]}))?;
    let cancelled = client.run(json!({"op":"cancelMarquee","revision":revision}))?;
    assert!(cancelled["marquee"].is_null());
    assert_eq!(cancelled["selections"], finished["selections"]);
    client.run(json!({"op":"beginMarquee","revision":revision,"frame":2,"point":[10,8]}))?;
    let clicked = client.run(json!({"op":"finishMarquee","revision":revision,"point":[11,8]}))?;
    assert_eq!(clicked["selections"], json!([selected]));

    assert_eq!(client.pixel(2)?, json!([0, 255, 0, 255]));
    assert_eq!(client.pixel(6)?, json!([0, 0, 255, 255]));
    let disabled = client.set_v2(false)?;
    assert_eq!(disabled["state"]["dirty"], true);
    assert_eq!(disabled["state"]["canUndo"], true);
    client.set_v2(false)?;
    assert_eq!(client.state()?["revision"], disabled["state"]["revision"]);
    assert_eq!(
        disabled["change"]["damage"]["picture"]["ranges"],
        json!([[0, 4], [8, 12]])
    );
    assert_eq!(disabled["change"]["damage"]["audio"]["kind"], "none");
    assert_eq!(fs::read_to_string(&master)?, original);
    assert_eq!(client.pixel(2)?, json!([255, 0, 0, 255]));
    assert_eq!(client.pixel(6)?, json!([0, 0, 255, 255]));
    let hidden = client.run(json!({
        "op":"hitTest", "revision":disabled["state"]["revision"], "frame":2, "point":[10,8]
    }))?;
    assert_eq!(
        hidden["hits"],
        json!([{"occurrence":"V1:plate@0","layers":[0]}])
    );
    let revision = &disabled["state"]["revision"];
    client.run(json!({"op":"beginMarquee","revision":revision,"frame":2,"point":[0,0]}))?;
    let finished = client.run(json!({"op":"finishMarquee","revision":revision,"point":[30,24]}))?;
    assert_eq!(
        finished["selections"],
        json!([{"kind":"layer","occurrence":"V1:plate@0","layers":[0]}])
    );

    let undone = client.run(json!({"op":"undo","revision":disabled["state"]["revision"]}))?;
    assert_eq!(undone["state"]["dirty"], false);
    assert_eq!(client.pixel(2)?, json!([0, 255, 0, 255]));
    let redone = client.run(json!({"op":"redo","revision":undone["state"]["revision"]}))?;
    client.run(json!({"op":"save","revision":redone["state"]["revision"]}))?;
    assert_eq!(client.state()?["dirty"], false);
    assert_eq!(
        fs::read_to_string(&master)?,
        original.replacen("\r\n", "\r\ntrack V2 off\r\n", 1)
    );
    let after_save = client.run(json!({"op":"undo","revision":redone["state"]["revision"]}))?;
    assert_eq!(after_save["state"]["dirty"], true);
    let back_to_save =
        client.run(json!({"op":"redo","revision":after_save["state"]["revision"]}))?;
    assert_eq!(back_to_save["state"]["dirty"], false);
    client.close()?;

    let mut reopened = Client::open(&master)?;
    let state = reopened.state()?;
    let root_v2 = state["tracks"]
        .as_array()
        .context("missing tracks")?
        .iter()
        .find(|lane| lane["scope"] == "root" && lane["track"] == "V2")
        .context("missing root V2")?;
    assert_eq!(root_v2["enabled"], false);
    assert_eq!(reopened.pixel(2)?, json!([255, 0, 0, 255]));
    assert_eq!(reopened.pixel(6)?, json!([0, 0, 255, 255]));

    reopened.set_v2(true)?;
    let external = fs::read_to_string(&master)?.replace("[0,4) V2 badge", "[0,3) V2 badge");
    fs::write(&master, &external)?;
    assert_eq!(reopened.next()?["event"], "stateChanged");
    let conflicted = reopened.state()?;
    assert_eq!(conflicted["dirty"], true);
    assert_eq!(conflicted["conflict"], true);
    let rejected = reopened.request(json!({"op":"save","revision":conflicted["revision"]}))?;
    assert!(rejected.get("error").is_some(), "{rejected}");
    assert_eq!(fs::read_to_string(&master)?, external);
    assert_eq!(reopened.pixel(2)?, json!([0, 255, 0, 255]));
    reopened.run(json!({
        "op":"reload", "revision":conflicted["revision"], "discardChanges":true
    }))?;
    let reloaded = reopened.state()?;
    assert_eq!(reloaded["dirty"], false);
    assert_eq!(reloaded["conflict"], false);
    assert_eq!(reloaded["canUndo"], false);
    assert_eq!(reopened.pixel(2)?, json!([255, 0, 0, 255]));

    let picked = reopened.run(json!({
        "op":"selectAt", "revision":reloaded["revision"], "frame":2, "point":[10,8]
    }))?;
    assert_eq!(picked["selection"]["occurrence"], "V1:plate@0");
    let plate = sequences.join("plate.xml");
    let artwork = fs::read_to_string(&plate)?;
    let rect = artwork
        .split_once("<rect")
        .unwrap()
        .1
        .split_once("</sequence>")
        .unwrap()
        .0;
    fs::write(
        &plate,
        artwork.replace("</sequence>", &format!("<rect{rect}</sequence>")),
    )?;
    assert_eq!(reopened.next()?["event"], "stateChanged");
    assert!(reopened.state()?["selection"].is_null());
    reopened.close()
}
