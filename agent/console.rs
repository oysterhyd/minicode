use rustyline::{
    Cmd, Context, Editor, Helper, KeyEvent,
    completion::{Completer, FilenameCompleter, Pair},
    error::ReadlineError,
    highlight::Highlighter,
    hint::Hinter,
    history::DefaultHistory,
    validate::Validator,
};
use std::io::{self, IsTerminal};

#[derive(Default)]
struct ConsoleHelper;
impl Helper for ConsoleHelper {}
impl Highlighter for ConsoleHelper {}
impl Hinter for ConsoleHelper {
    type Hint = String;
}
impl Validator for ConsoleHelper {}
impl Completer for ConsoleHelper {
    type Candidate = Pair;
    fn complete(
        &self,
        line: &str,
        pos: usize,
        ctx: &Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Pair>)> {
        let prefix = &line[..pos];
        if prefix.starts_with('/') && !prefix.contains(char::is_whitespace) {
            let commands: Vec<serde_json::Value> =
                serde_json::from_str(include_str!("assets/commands.json")).unwrap_or_default();
            return Ok((
                0,
                commands
                    .iter()
                    .filter_map(|command| command["name"].as_str())
                    .filter(|name| name.starts_with(prefix))
                    .map(|name| Pair {
                        display: name.into(),
                        replacement: name.into(),
                    })
                    .collect(),
            ));
        }
        FilenameCompleter::new().complete(line, pos, ctx)
    }
}

pub fn input() -> tokio::sync::mpsc::UnboundedReceiver<io::Result<String>> {
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    // One detached editor owns stdin for its whole lifetime; dropping an async
    // read never creates a second reader or keeps Tokio's blocking pool alive.
    std::thread::spawn(move || {
        if io::stdin().is_terminal() && io::stdout().is_terminal() {
            let Ok(mut editor) = Editor::<ConsoleHelper, DefaultHistory>::new() else {
                let _ = sender.send(Err(io::Error::other("line editor initialization failed")));
                return;
            };
            editor.set_helper(Some(ConsoleHelper));
            editor.bind_sequence(KeyEvent::ctrl('j'), Cmd::Newline);
            let history = crate::util::data_dir().join("cli-history");
            let _ = editor.load_history(&history);
            loop {
                match editor.readline("❯ ") {
                    Ok(line) => {
                        let _ = editor.add_history_entry(line.as_str());
                        if let Some(parent) = history.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        let _ = editor.save_history(&history);
                        if sender.send(Ok(line)).is_err() {
                            break;
                        }
                    }
                    Err(ReadlineError::Interrupted) => {
                        if sender
                            .send(Err(io::Error::from(io::ErrorKind::Interrupted)))
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(ReadlineError::Eof) => break,
                    Err(error) => {
                        let _ = sender.send(Err(io::Error::other(error.to_string())));
                        break;
                    }
                }
            }
        } else {
            loop {
                let mut line = String::new();
                match io::stdin().read_line(&mut line) {
                    Ok(0) => break,
                    Ok(_) => {
                        if sender.send(Ok(line)).is_err() {
                            break;
                        }
                    }
                    Err(error) => {
                        let _ = sender.send(Err(error));
                        break;
                    }
                }
            }
        }
    });
    receiver
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustyline::history::{History, MemHistory};
    #[test]
    fn l13_editor_completes_commands_and_preserves_multiline_history() {
        let mut history = MemHistory::new();
        history.add("first\nsecond").unwrap();
        let context = Context::new(&history);
        let (_, items) = ConsoleHelper.complete("/res", 4, &context).unwrap();
        assert!(items.iter().any(|item| item.replacement == "/resume"));
        assert_eq!(history.len(), 1);
        assert_eq!(
            history
                .get(0, rustyline::history::SearchDirection::Forward)
                .unwrap()
                .unwrap()
                .entry,
            "first\nsecond"
        );
    }
}
