use futures::channel::mpsc;
use iced::Task;

/// 블로킹 작업을 전용 OS 스레드에서 돌리고 결과를 메시지 스트림으로 돌려준다.
///
/// `iced_runtime::task::blocking`과 같지만 직접 들고 있는다: `iced`는 이걸
/// 재수출하지 않고, `iced_runtime`을 따로 의존하면 버전이 어긋났을 때 서로
/// 호환되지 않는 `Task` 타입이 두 개 생긴다.
pub fn blocking<T>(f: impl FnOnce(mpsc::Sender<T>) + Send + 'static) -> Task<T>
where
    T: Send + 'static,
{
    let (sender, receiver) = mpsc::channel(1);
    std::thread::spawn(move || f(sender));
    Task::stream(receiver)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc as std_mpsc;
    use std::time::Duration;

    #[test]
    fn blocking_body_runs_off_the_calling_thread() {
        let (tx, rx) = std_mpsc::channel();
        let caller = std::thread::current().id();
        let _task = blocking(move |_out: futures::channel::mpsc::Sender<()>| {
            tx.send(std::thread::current().id()).unwrap();
        });
        let ran_on = rx.recv_timeout(Duration::from_secs(5)).expect("body ran");
        assert_ne!(
            ran_on, caller,
            "blocking body must not run on the caller thread"
        );
    }
}
