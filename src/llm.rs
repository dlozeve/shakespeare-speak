//! OpenRouter client.
//!
//! The model is used for exactly two things, and neither can put text into the
//! answer:
//!
//! 1. `propose` — suggest Shakespearean *wordings* for a modern phrase.  Every
//!    suggestion is treated as untrusted and must be found verbatim in the
//!    local corpus before it is shown.
//! 2. `rank` — order a list of quotations that have *already* been verified.
//!    The model can only return indices, so it cannot invent a quotation or a
//!    citation.

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};

const PROPOSE_SYSTEM: &str = "\
You are a Shakespeare concordance assistant. The user gives a modern English \
phrase. Propose short phrases (2 to 8 words) that Shakespeare actually wrote \
and that express the same meaning, best match first.

Rules:
- Only propose wordings you believe appear verbatim in Shakespeare's plays or poems.
- Give the exact wording, without quotation marks, citations, act/scene references, \
translations, or explanations.
- Prefer distinctive, quotable lines — the kind a person would recognise as the \
Shakespearean version of the phrase — over generic English that merely happens to \
appear in the plays. \"At one fell swoop\" is a good candidate; \"in a moment\" is not.
- If you cannot think of a genuine Shakespearean equivalent, return an empty list.

Reply with JSON only, in this shape:
{\"candidates\": [\"phrase one\", \"phrase two\"]}";

const RANK_SYSTEM: &str = "\
You are a Shakespeare scholar. You are given a modern English phrase and a \
numbered list of quotations that have already been verified to exist verbatim in \
Shakespeare's works. Each entry also says how many times that wording occurs in \
the complete works.

Rank the quotations from best to worst at expressing the same meaning as the \
modern phrase. Prefer, in this order:

1. A distinctive, memorable Shakespearean idiom that a modern speaker would \
   recognise as the Shakespearean version of the phrase.
2. A quotation whose meaning is closest to the modern phrase.
3. A quotation that is recognisably Shakespearean rather than ordinary English \
   that merely happens to appear in the plays.

A wording that occurs many times is usually ordinary English, not an idiom, so \
do not rank a generic phrase highly just because it is common. A wording that \
occurs once or twice is more likely to be the distinctive line the user wants.

Also rank down a quotation that is a fragment: one that begins or ends \
mid-sentence, or that is a stretch of ordinary prose rather than a self-contained \
line. A short, complete, memorable passage beats a longer broken one.

You may only return indices from the list, best first. Never invent or rewrite a \
quotation.

Reply with JSON only, in this shape:
{\"ranking\": [2, 0, 1]}";

pub struct Client {
    key: String,
    pub model: String,
    base_url: String,
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<Message>,
    temperature: f32,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<ResponseFormat>,
}

#[derive(Serialize)]
struct Message {
    role: &'static str,
    content: String,
}

#[derive(Serialize)]
struct ResponseFormat {
    #[serde(rename = "type")]
    kind: &'static str,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: ChoiceMessage,
}

#[derive(Deserialize)]
struct ChoiceMessage {
    content: Option<String>,
}

impl Client {
    pub fn new(key: String, model: String, base_url: String) -> Self {
        Client {
            key,
            model,
            base_url,
        }
    }

    /// Ask for candidate Shakespearean wordings, best first.
    pub fn propose(&self, query: &str, count: usize) -> Result<Vec<String>> {
        let user =
            format!("Modern phrase: {query:?}\nReturn up to {count} candidates, best match first.");
        let raw = self.complete(PROPOSE_SYSTEM, &user)?;
        let value = extract_json(&raw)
            .ok_or_else(|| anyhow!("model reply was not JSON: {}", truncate(&raw)))?;

        let array = value
            .get("candidates")
            .and_then(|candidates| candidates.as_array())
            .or_else(|| value.as_array())
            .ok_or_else(|| anyhow!("model reply had no `candidates` array: {}", truncate(&raw)))?;

        let mut out = Vec::new();
        for item in array {
            if let Some(text) = item.as_str() {
                let text = text.trim().trim_matches('"').trim();
                if !text.is_empty() {
                    out.push(text.to_string());
                }
            }
        }
        out.truncate(count);
        Ok(out)
    }

    /// Order quotations that are already verified, best first.
    ///
    /// `occurrences` is how many times each wording appears in the corpus, which
    /// the model uses to prefer distinctive idioms over ordinary English.
    ///
    /// The result is always a full permutation of `0..quotes.len()`: indices the
    /// model invents are dropped, and any it forgets are appended in their
    /// original order.
    pub fn rank(
        &self,
        query: &str,
        quotes: &[String],
        occurrences: &[usize],
    ) -> Result<Vec<usize>> {
        if quotes.len() < 2 {
            return Ok((0..quotes.len()).collect());
        }

        let mut user = format!("Modern phrase: {query:?}\n\nQuotations:\n");
        for (index, quote) in quotes.iter().enumerate() {
            let count = occurrences.get(index).copied().unwrap_or(1);
            let times = if count == 1 {
                "occurs once".to_string()
            } else {
                format!("occurs {count} times")
            };
            user.push_str(&format!("{index}. {quote}  ({times})\n"));
        }
        user.push_str("\nReturn every index, best first.");

        let raw = self.complete(RANK_SYSTEM, &user)?;
        let value = extract_json(&raw)
            .ok_or_else(|| anyhow!("model reply was not JSON: {}", truncate(&raw)))?;

        let listed = value
            .get("ranking")
            .and_then(|ranking| ranking.as_array())
            .or_else(|| value.as_array())
            .ok_or_else(|| anyhow!("model reply had no `ranking` array: {}", truncate(&raw)))?;

        Ok(normalise_ranking(listed, quotes.len()))
    }

    fn complete(&self, system: &str, user: &str) -> Result<String> {
        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));

        // Ask for JSON mode first; some providers reject it, so retry plainly.
        match self.send(&url, system, user, true) {
            Ok(text) => Ok(text),
            Err(Attempt::Rejected) => self
                .send(&url, system, user, false)
                .map_err(|_| anyhow!("provider rejected the request (JSON mode and plain)")),
            Err(Attempt::Failed(message)) => Err(anyhow!(message)),
        }
    }

    fn send(
        &self,
        url: &str,
        system: &str,
        user: &str,
        json_mode: bool,
    ) -> std::result::Result<String, Attempt> {
        let body = ChatRequest {
            model: &self.model,
            messages: vec![
                Message {
                    role: "system",
                    content: system.to_string(),
                },
                Message {
                    role: "user",
                    content: user.to_string(),
                },
            ],
            temperature: 0.2,
            max_tokens: 800,
            response_format: json_mode.then_some(ResponseFormat {
                kind: "json_object",
            }),
        };

        let response = ureq::post(url)
            .set("Authorization", &format!("Bearer {}", self.key))
            .set("Content-Type", "application/json")
            .set("HTTP-Referer", "https://github.com/local/shakespeare-speak")
            .set("X-Title", "shakespeare-speak")
            .timeout(std::time::Duration::from_secs(60))
            .send_json(&body);

        let response = match response {
            Ok(response) => response,
            Err(ureq::Error::Status(code, response)) => {
                let detail = response.into_string().unwrap_or_default();
                let detail = truncate(detail.trim());
                // 400/404/422 usually mean the model or JSON mode is unsupported.
                return if matches!(code, 400 | 404 | 422) {
                    Err(Attempt::Rejected)
                } else {
                    Err(Attempt::Failed(format!(
                        "provider returned HTTP {code}: {detail}"
                    )))
                };
            }
            Err(error) => return Err(Attempt::Failed(format!("request failed: {error}"))),
        };

        let parsed: ChatResponse = response
            .into_json()
            .context("decoding provider response")
            .map_err(|error| Attempt::Failed(error.to_string()))?;

        parsed
            .choices
            .into_iter()
            .next()
            .and_then(|choice| choice.message.content)
            .filter(|content| !content.trim().is_empty())
            .ok_or_else(|| Attempt::Failed("provider returned an empty reply".to_string()))
    }
}

enum Attempt {
    /// Retryable without JSON mode (unsupported model or parameter).
    Rejected,
    Failed(String),
}

/// Turn the model's ranking into a full permutation of `0..len`.
///
/// Invented and out-of-range indices are dropped, duplicates are dropped, and
/// anything the model forgot is appended in its original order — so a bad reply
/// degrades to the propose order instead of losing quotations.
fn normalise_ranking(listed: &[serde_json::Value], len: usize) -> Vec<usize> {
    let mut order: Vec<usize> = Vec::with_capacity(len);

    for item in listed {
        let index = match item {
            serde_json::Value::Number(number) => number.as_u64().map(|n| n as usize),
            serde_json::Value::String(text) => text.trim().parse().ok(),
            _ => None,
        };
        if let Some(index) = index {
            if index < len && !order.contains(&index) {
                order.push(index);
            }
        }
    }

    for index in 0..len {
        if !order.contains(&index) {
            order.push(index);
        }
    }

    order
}

/// Pull the first JSON object or array out of a reply, tolerating prose or
/// fenced code blocks around it.
fn extract_json(text: &str) -> Option<serde_json::Value> {
    let candidates = [
        (text.find('{'), text.rfind('}')),
        (text.find('['), text.rfind(']')),
    ];

    for (start, end) in candidates {
        if let (Some(start), Some(end)) = (start, end) {
            if start < end {
                if let Ok(value) = serde_json::from_str(&text[start..=end]) {
                    return Some(value);
                }
            }
        }
    }
    None
}

fn truncate(text: &str) -> String {
    const LIMIT: usize = 300;
    if text.chars().count() <= LIMIT {
        return text.to_string();
    }
    let mut out: String = text.chars().take(LIMIT).collect();
    out.push('…');
    out
}

/// Fail early with a helpful message when no key is configured.
pub fn api_key() -> Result<String> {
    match std::env::var("OPENROUTER_API_KEY") {
        Ok(key) if !key.trim().is_empty() => Ok(key.trim().to_string()),
        _ => bail!(
            "OPENROUTER_API_KEY is not set.\n\
             Set it, or run with --search to query the corpus without a model."
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::{extract_json, normalise_ranking};
    use serde_json::json;

    #[test]
    fn keeps_a_valid_ranking() {
        let listed = [json!(2), json!(0), json!(1)];
        assert_eq!(normalise_ranking(&listed, 3), vec![2, 0, 1]);
    }

    #[test]
    fn drops_invented_and_out_of_range_indices() {
        let listed = [json!(1), json!(99), json!(-3), json!("x"), json!(0)];
        assert_eq!(normalise_ranking(&listed, 2), vec![1, 0]);
    }

    #[test]
    fn drops_duplicates() {
        let listed = [json!(1), json!(1), json!(0)];
        assert_eq!(normalise_ranking(&listed, 2), vec![1, 0]);
    }

    #[test]
    fn appends_anything_the_model_forgot() {
        let listed = [json!(2)];
        assert_eq!(normalise_ranking(&listed, 4), vec![2, 0, 1, 3]);
    }

    #[test]
    fn an_empty_ranking_degrades_to_propose_order() {
        assert_eq!(normalise_ranking(&[], 3), vec![0, 1, 2]);
    }

    #[test]
    fn accepts_numeric_strings() {
        let listed = [json!("1"), json!("0")];
        assert_eq!(normalise_ranking(&listed, 2), vec![1, 0]);
    }

    #[test]
    fn extracts_json_from_prose_and_fences() {
        let value = extract_json("```json\n{\"candidates\": [\"a\"]}\n```").unwrap();
        assert_eq!(value["candidates"][0], "a");

        let value = extract_json("Sure! {\"choice\": 2} hope that helps").unwrap();
        assert_eq!(value["choice"], 2);

        let value = extract_json("[\"a\", \"b\"]").unwrap();
        assert_eq!(value[1], "b");
    }

    #[test]
    fn rejects_non_json() {
        assert!(extract_json("I cannot help with that.").is_none());
    }
}
