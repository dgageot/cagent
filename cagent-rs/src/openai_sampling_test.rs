use crate::chat::Message;
use crate::model::openai::OpenAIProvider;
use crate::model::ProviderOptions;

#[test]
fn openai_request_includes_sampling_fields() {
    let provider = OpenAIProvider::new(
        "openai".to_string(),
        "test".to_string(),
        "gpt-4o".to_string(),
        ProviderOptions {
            temperature: Some(0.7),
            top_p: Some(0.9),
            frequency_penalty: Some(0.1),
            presence_penalty: Some(0.2),
            ..Default::default()
        },
        None,
    );

    let body = provider.build_request_body_for_test(&[Message::user("hi")], &[]);

    let top_p = body
        .get("top_p")
        .and_then(|v| v.as_f64())
        .expect("missing top_p");
    assert!((top_p - 0.9).abs() < 1e-6, "unexpected top_p: {top_p}");

    let freq = body
        .get("frequency_penalty")
        .and_then(|v| v.as_f64())
        .expect("missing frequency_penalty");
    assert!(
        (freq - 0.1).abs() < 1e-6,
        "unexpected frequency_penalty: {freq}"
    );

    let pres = body
        .get("presence_penalty")
        .and_then(|v| v.as_f64())
        .expect("missing presence_penalty");
    assert!(
        (pres - 0.2).abs() < 1e-6,
        "unexpected presence_penalty: {pres}"
    );
}
