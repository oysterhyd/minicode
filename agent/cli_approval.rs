#[derive(Debug, PartialEq)]
pub struct Decision {
    pub granted: bool,
    pub remember: &'static str,
}

pub fn reply(line: &str, nonce: &str, can_remember: bool) -> Option<Decision> {
    let words = line.split_whitespace().collect::<Vec<_>>();
    match words.as_slice() {
        [answer] if answer.eq_ignore_ascii_case("n") || answer.eq_ignore_ascii_case("no") => {
            Some(Decision {
                granted: false,
                remember: "",
            })
        }
        [answer, code] if code.eq_ignore_ascii_case(nonce) => {
            let remember = match answer.to_ascii_lowercase().as_str() {
                "y" | "yes" => "",
                "a" if can_remember => "a",
                "p" if can_remember => "p",
                _ => return None,
            };
            Some(Decision {
                granted: true,
                remember,
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn l12_only_a_fresh_confirmation_grants() {
        for text in [
            "y",
            "yes",
            "a",
            "p",
            "y wrong",
            "ordinary pending message",
            "y 12345678 extra",
        ] {
            assert_eq!(reply(text, "12345678", true), None);
        }
        assert!(reply("y 12345678", "12345678", false).unwrap().granted);
        assert_eq!(
            reply(" a\t12345678 ", "12345678", true).unwrap().remember,
            "a"
        );
        assert_eq!(reply("a 12345678", "12345678", false), None);
        assert!(!reply("n", "12345678", false).unwrap().granted);
    }
}
