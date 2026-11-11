use lms_api::LmStudio;
use mockito::Server;
use url::Url;

#[tokio::test]
async fn test_models_list() {
    let mut server = Server::new_async().await;
    let url = server.url();

    let _m = server.mock("GET", "/api/v1/models")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"models": [{"type": "llm", "publisher": "test", "key": "test-model", "display_name": "Test Model", "size_bytes": 1024, "loaded_instances": [], "max_context_length": 2048}]}"#)
        .create_async()
        .await;

    let client = LmStudio::from_url(Url::parse(&url).unwrap());
    let models = client.models().await.unwrap();

    assert_eq!(models.len(), 1);
    assert_eq!(models[0].key, "test-model");
}

#[tokio::test]
async fn test_error_handling() {
    let mut server = Server::new_async().await;
    let url = server.url();

    let _m = server
        .mock("GET", "/api/v1/models")
        .with_status(500)
        .with_body("Internal Server Error")
        .create_async()
        .await;

    let client = LmStudio::from_url(Url::parse(&url).unwrap());
    let result = client.models().await;

    assert!(result.is_err());
    if let Err(lms_api::Error::Server { status, body }) = result {
        assert_eq!(status, 500);
        assert_eq!(body, "Internal Server Error");
    } else {
        panic!("Expected Server error");
    }
}

#[cfg(feature = "stream")]
#[tokio::test]
async fn test_chat_stream_error_event() {
    use lms_api::chat::request::ChatRequestBuilder;
    use lms_api::chat::stream::response::{ErrorType, StreamEvent};
    use tokio_stream::StreamExt;

    let mut server = Server::new_async().await;
    let url = server.url();

    let _m = server
        .mock("POST", "/api/v1/chat")
        .with_status(200)
        .with_header("content-type", "text/event-stream")
        .with_body(concat!(
            "event: error\n",
            r#"data: {"type":"error","error":{"type":"model_not_found","message":"no such model"}}"#,
            "\n\n",
        ))
        .create_async()
        .await;

    let client = LmStudio::from_url(Url::parse(&url).unwrap());
    let request = ChatRequestBuilder::default()
        .model("missing")
        .input("hi")
        .build()
        .unwrap();
    let mut stream = client.chat_stream(request).await.unwrap();

    match stream.next().await {
        Some(Ok(StreamEvent::Error { error })) => {
            assert!(matches!(error.error_type, ErrorType::ModelNotFound));
            assert_eq!(error.message, "no such model");
        }
        other => panic!("expected an error event, got {other:?}"),
    }
}

#[test]
fn test_invalid_host_keeps_its_cause() {
    let err = match LmStudio::new("unix:/tmp/lms.sock", 1234) {
        Err(err) => err,
        Ok(_) => panic!("expected an invalid host error"),
    };

    assert!(matches!(err, lms_api::Error::Host(_)), "{err:?}");
    assert!(std::error::Error::source(&err).is_some());
}

#[tokio::test]
async fn test_chat_returns_the_response() {
    use lms_api::chat::request::ChatRequestBuilder;
    use lms_api::chat::response::Output;
    use mockito::Matcher;

    let mut server = Server::new_async().await;
    let url = server.url();

    let _m = server
        .mock("POST", "/api/v1/chat")
        .match_body(Matcher::PartialJson(serde_json::json!({
            "model": "test-model",
            "input": "hi",
        })))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"{"model_instance_id":"test-model","output":[{"type":"reasoning","content":"thinking"},{"type":"message","content":"feat: add tests"}],"stats":{"input_tokens":3,"total_output_tokens":5,"reasoning_output_tokens":1,"tokens_per_second":10.5,"time_to_first_token_seconds":0.1},"response_id":"resp_1"}"#,
        )
        .create_async()
        .await;

    let client = LmStudio::from_url(Url::parse(&url).unwrap());
    let request = ChatRequestBuilder::default()
        .model("test-model")
        .input("hi")
        .build()
        .unwrap();
    let response = client.chat(request).await.unwrap();

    assert_eq!(response.model_instance_id, "test-model");
    assert_eq!(response.response_id.as_deref(), Some("resp_1"));
    assert!(response.stats.model_load_time_seconds.is_none());
    match &response.output[..] {
        [
            Output::Reasoning { content: reasoning },
            Output::Message { content: message },
        ] => {
            assert_eq!(reasoning, "thinking");
            assert_eq!(message, "feat: add tests");
        }
        other => panic!("unexpected output {other:?}"),
    }
}

#[tokio::test]
async fn test_chat_server_error_keeps_the_body() {
    use lms_api::chat::request::ChatRequestBuilder;

    let mut server = Server::new_async().await;
    let url = server.url();

    let _m = server
        .mock("POST", "/api/v1/chat")
        .with_status(400)
        .with_body(r#"{"error":{"message":"bad request"}}"#)
        .create_async()
        .await;

    let client = LmStudio::from_url(Url::parse(&url).unwrap());
    let request = ChatRequestBuilder::default()
        .model("test-model")
        .input("hi")
        .build()
        .unwrap();

    match client.chat(request).await {
        Err(lms_api::Error::Server { status, body }) => {
            assert_eq!(status, 400);
            assert_eq!(body, r#"{"error":{"message":"bad request"}}"#);
        }
        other => panic!("expected a server error, got {other:?}"),
    }
}

#[cfg(feature = "stream")]
#[tokio::test]
async fn test_chat_stream_yields_deltas_until_done() {
    use lms_api::chat::request::ChatRequestBuilder;
    use lms_api::chat::stream::response::StreamEvent;
    use mockito::Matcher;
    use tokio_stream::StreamExt;

    let mut server = Server::new_async().await;
    let url = server.url();

    let _m = server
        .mock("POST", "/api/v1/chat")
        .match_body(Matcher::PartialJson(serde_json::json!({ "stream": true })))
        .with_status(200)
        .with_header("content-type", "text/event-stream")
        .with_body(concat!(
            "event: message.delta\n",
            r#"data: {"type":"message.delta","content":"feat: "}"#,
            "\n\n",
            "event: message.delta\n",
            r#"data: {"type":"message.delta","content":"한글"}"#,
            "\n\n",
            "data: [DONE]\n\n",
            r#"data: {"type":"message.delta","content":"ignored"}"#,
            "\n\n",
        ))
        .create_async()
        .await;

    let client = LmStudio::from_url(Url::parse(&url).unwrap());
    let request = ChatRequestBuilder::default()
        .model("test-model")
        .input("hi")
        .build()
        .unwrap();
    let events: Vec<_> = client.chat_stream(request).await.unwrap().collect().await;

    let contents: Vec<_> = events
        .into_iter()
        .map(|event| match event {
            Ok(StreamEvent::MessageDelta { content }) => content,
            other => panic!("expected a message delta, got {other:?}"),
        })
        .collect();
    assert_eq!(contents, ["feat: ", "한글"]);
}
