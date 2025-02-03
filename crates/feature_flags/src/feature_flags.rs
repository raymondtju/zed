use futures::channel::oneshot;
use futures::{select_biased, FutureExt};
use gpui::{App, Context, Global, Subscription, Task, Window};
use std::time::Duration;
use std::{future::Future, pin::Pin, task::Poll};

#[derive(Default)]
struct FeatureFlags {
    flags: Vec<String>,
    staff: bool,
}

impl FeatureFlags {
    fn has_flag<T: FeatureFlag>(&self) -> bool {
        if self.staff && T::enabled_for_staff() {
            return true;
        }

        self.flags.iter().any(|f| f.as_str() == T::NAME)
    }
}

impl Global for FeatureFlags {}

/// To create a feature flag, implement this trait on a trivial type and use it as
/// a generic parameter when called [`FeatureFlagAppExt::has_flag`].
///
/// Feature flags are enabled for members of Zed staff by default. To disable this behavior
/// so you can test flags being disabled, set ZED_DISABLE_STAFF=1 in your environment,
/// which will force Zed to treat the current user as non-staff.
pub trait FeatureFlag {
    const NAME: &'static str;

    /// Returns whether this feature flag is enabled for Zed staff.
    fn enabled_for_staff() -> bool {
        true
    }
}

pub struct Assistant2FeatureFlag;

impl FeatureFlag for Assistant2FeatureFlag {
    const NAME: &'static str = "assistant2";
}

pub struct PredictEditsFeatureFlag;
impl FeatureFlag for PredictEditsFeatureFlag {
    const NAME: &'static str = "predict-edits";
}

pub struct PredictEditsRateCompletionsFeatureFlag;
impl FeatureFlag for PredictEditsRateCompletionsFeatureFlag {
    const NAME: &'static str = "predict-edits-rate-completions";
}

pub struct GitUiFeatureFlag;
impl FeatureFlag for GitUiFeatureFlag {
    const NAME: &'static str = "git-ui";
}

pub struct Remoting {}
impl FeatureFlag for Remoting {
    const NAME: &'static str = "remoting";
}

pub struct LanguageModels {}
impl FeatureFlag for LanguageModels {
    const NAME: &'static str = "language-models";
}

pub struct LlmClosedBeta {}
impl FeatureFlag for LlmClosedBeta {
    const NAME: &'static str = "llm-closed-beta";
}

pub struct ZedPro {}
impl FeatureFlag for ZedPro {
    const NAME: &'static str = "zed-pro";
}

pub struct NotebookFeatureFlag;

impl FeatureFlag for NotebookFeatureFlag {
    const NAME: &'static str = "notebooks";
}

pub struct AutoCommand {}
impl FeatureFlag for AutoCommand {
    const NAME: &'static str = "auto-command";

    fn enabled_for_staff() -> bool {
        false
    }
}

pub trait FeatureFlagViewExt<V: 'static> {
    fn observe_flag<T: FeatureFlag, F>(&mut self, window: &Window, callback: F) -> Subscription
    where
        F: Fn(bool, &mut V, &mut Window, &mut Context<V>) + Send + Sync + 'static;
}

impl<V> FeatureFlagViewExt<V> for Context<'_, V>
where
    V: 'static,
{
    fn observe_flag<T: FeatureFlag, F>(&mut self, window: &Window, callback: F) -> Subscription
    where
        F: Fn(bool, &mut V, &mut Window, &mut Context<V>) + 'static,
    {
        self.observe_global_in::<FeatureFlags>(window, move |v, window, cx| {
            let feature_flags = cx.global::<FeatureFlags>();
            callback(feature_flags.has_flag::<T>(), v, window, cx);
        })
    }
}

pub trait FeatureFlagAppExt {
    fn wait_for_flag<T: FeatureFlag>(&mut self) -> WaitForFlag;

    /// Waits for the specified feature flag to resolve, up to the given timeout.
    fn wait_for_flag_or_timeout<T: FeatureFlag>(&mut self, timeout: Duration) -> Task<bool>;

    fn update_flags(&mut self, staff: bool, flags: Vec<String>);
    fn set_staff(&mut self, staff: bool);
    fn has_flag<T: FeatureFlag>(&self) -> bool;
    fn is_staff(&self) -> bool;

    fn observe_flag<T: FeatureFlag, F>(&mut self, callback: F) -> Subscription
    where
        F: FnMut(bool, &mut App) + 'static;
}

impl FeatureFlagAppExt for App {
    fn update_flags(&mut self, staff: bool, flags: Vec<String>) {
        let feature_flags = self.default_global::<FeatureFlags>();
        feature_flags.staff = staff;
        feature_flags.flags = flags;
    }

    fn set_staff(&mut self, staff: bool) {
        let feature_flags = self.default_global::<FeatureFlags>();
        feature_flags.staff = staff;
    }

    fn has_flag<T: FeatureFlag>(&self) -> bool {
        self.try_global::<FeatureFlags>()
            .map(|flags| flags.has_flag::<T>())
            .unwrap_or(false)
    }

    fn is_staff(&self) -> bool {
        self.try_global::<FeatureFlags>()
            .map(|flags| flags.staff)
            .unwrap_or(false)
    }

    fn observe_flag<T: FeatureFlag, F>(&mut self, mut callback: F) -> Subscription
    where
        F: FnMut(bool, &mut App) + 'static,
    {
        self.observe_global::<FeatureFlags>(move |cx| {
            let feature_flags = cx.global::<FeatureFlags>();
            callback(feature_flags.has_flag::<T>(), cx);
        })
    }

    fn wait_for_flag<T: FeatureFlag>(&mut self) -> WaitForFlag {
        let (tx, rx) = oneshot::channel::<bool>();
        let mut tx = Some(tx);
        let subscription: Option<Subscription>;

        match self.try_global::<FeatureFlags>() {
            Some(feature_flags) => {
                subscription = None;
                tx.take().unwrap().send(feature_flags.has_flag::<T>()).ok();
            }
            None => {
                subscription = Some(self.observe_global::<FeatureFlags>(move |cx| {
                    let feature_flags = cx.global::<FeatureFlags>();
                    if let Some(tx) = tx.take() {
                        tx.send(feature_flags.has_flag::<T>()).ok();
                    }
                }));
            }
        }

        WaitForFlag(rx, subscription)
    }

    fn wait_for_flag_or_timeout<T: FeatureFlag>(&mut self, timeout: Duration) -> Task<bool> {
        let wait_for_flag = self.wait_for_flag::<T>();

        self.spawn(|_cx| async move {
            let mut wait_for_flag = wait_for_flag.fuse();
            let mut timeout = FutureExt::fuse(smol::Timer::after(timeout));

            select_biased! {
                is_enabled = wait_for_flag => is_enabled,
                _ = timeout => false,
            }
        })
    }
}

pub struct WaitForFlag(oneshot::Receiver<bool>, Option<Subscription>);

impl Future for WaitForFlag {
    type Output = bool;

    fn poll(mut self: Pin<&mut Self>, cx: &mut core::task::Context<'_>) -> Poll<Self::Output> {
        self.0.poll_unpin(cx).map(|result| {
            self.1.take();
            result.unwrap_or(false)
        })
    }
}
