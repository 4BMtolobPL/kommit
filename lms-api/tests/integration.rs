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
    if let Err(lms_api::error::ApiError::Status(status, body)) = result {
        assert_eq!(status, 500);
        assert_eq!(body, "Internal Server Error");
    } else {
        panic!("Expected Status error");
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
