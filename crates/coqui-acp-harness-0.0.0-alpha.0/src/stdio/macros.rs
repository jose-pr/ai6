//! The two providers a stdio harness is offered as.

/// Defines a harness's provider of `acp_harness` and its provider of `account_provider` over a
/// [`HarnessDef`](crate::stdio::accounts::HarnessDef), each under an id of its own.
///
/// ```ignore
/// coqui_acp_harness::stdio_vendor! {
///     harness: ExampleProvider, id: "example",
///     accounts: ExampleAccounts, id: "example_accounts",
///     def: DEF,
///     settings: StdioSettings, account_settings: StdioAccountSettings,
/// }
/// ```
///
/// `harness` and `accounts` name the two types it defines and `id` the id each is registered under. The provider of
/// `acp_harness` runs `def` through [`StdioProvider`](crate::stdio::StdioProvider), and its description names the accounts
/// provider as its link, sharing `executable`. `settings` and `account_settings` are the configurations the two are built
/// from: each has a `Default` and converts into [`StdioSettings`](crate::stdio::StdioSettings) and
/// [`StdioAccountSettings`](crate::stdio::StdioAccountSettings), so a harness that fixes some of their values has types of its
/// own. The crate that calls it names `coqui_acp_api` and `guatiao` as dependencies, as every provider's crate does.
#[macro_export]
macro_rules! stdio_vendor {
    (
        $(#[$harness_meta:meta])*
        harness: $harness:ident, id: $harness_id:literal,
        $(#[$accounts_meta:meta])*
        accounts: $accounts:ident, id: $accounts_id:literal,
        def: $def:expr,
        settings: $settings:ty, account_settings: $account_settings:ty $(,)?
    ) => {
        $(#[$harness_meta])*
        #[derive($crate::__private::api::guatiao::Provider, ::core::fmt::Debug)]
        #[provider(
            $crate::__private::api::AcpHarness,
            id = $harness_id,
            config = $settings,
            new = $harness::unconfigured,
            crate = $crate::__private::api::guatiao
        )]
        pub struct $harness {
            inner: $crate::stdio::StdioProvider,
        }

        impl ::core::convert::From<$settings> for $harness {
            fn from(settings: $settings) -> Self {
                Self {
                    inner: $crate::stdio::StdioProvider::new(&$def, settings.into()),
                }
            }
        }

        impl $harness {
            /// A provider configured as `settings` say.
            #[must_use]
            pub fn new(settings: $settings) -> Self {
                settings.into()
            }

            fn unconfigured() -> Self {
                Self::new(<$settings as ::core::default::Default>::default())
            }
        }

        impl $crate::__private::api::AcpHarness for $harness {
            fn describe(
                &self,
            ) -> ::core::result::Result<
                $crate::__private::api::HarnessInfo,
                $crate::__private::api::guatiao::library::ProviderError,
            > {
                let mut info = $crate::__private::api::AcpHarness::describe(&self.inner)?;
                info.accounts = ::core::option::Option::Some(
                    $crate::__private::api::AccountsLink::new($accounts_id).sharing(["executable"]),
                );
                ::core::result::Result::Ok(info)
            }

            fn accounts(
                &self,
            ) -> ::core::result::Result<
                $crate::__private::api::AccountCatalog,
                $crate::__private::api::guatiao::library::ProviderError,
            > {
                $crate::__private::api::AcpHarness::accounts(&self.inner)
            }

            fn start(
                &self,
                request: $crate::__private::api::StartRequest,
                spawner: $crate::__private::api::guatiao::library::Object<
                    dyn $crate::__private::api::ProcessSpawner,
                >,
                sink: $crate::__private::api::guatiao::library::Object<
                    dyn $crate::__private::api::ChannelSink,
                >,
            ) -> ::core::result::Result<
                $crate::__private::api::guatiao::library::Object<
                    dyn $crate::__private::api::Channel,
                >,
                $crate::__private::api::guatiao::library::ProviderError,
            > {
                $crate::__private::api::AcpHarness::start(&self.inner, request, spawner, sink)
            }

            fn refusal(
                &self,
                requirements: $crate::__private::alloc::vec::Vec<$crate::__private::api::Requirement>,
                environment: $crate::__private::api::guatiao::Secret<
                    $crate::__private::api::Environment,
                >,
                account: ::core::option::Option<$crate::__private::api::AccountRef>,
            ) -> ::core::result::Result<
                ::core::option::Option<$crate::__private::alloc::string::String>,
                $crate::__private::api::guatiao::library::ProviderError,
            > {
                $crate::__private::api::AcpHarness::refusal(&self.inner, requirements, environment, account)
            }

            fn program(
                &self,
            ) -> ::core::result::Result<
                ::core::option::Option<$crate::__private::alloc::string::String>,
                $crate::__private::api::guatiao::library::ProviderError,
            > {
                $crate::__private::api::AcpHarness::program(&self.inner)
            }

            fn import_session(
                &self,
                session: $crate::__private::api::SessionImport,
                spawner: $crate::__private::api::guatiao::library::Object<
                    dyn $crate::__private::api::ProcessSpawner,
                >,
            ) -> ::core::result::Result<
                $crate::__private::api::ImportReport,
                $crate::__private::api::guatiao::library::ProviderError,
            > {
                $crate::__private::api::AcpHarness::import_session(&self.inner, session, spawner)
            }

            fn export_session(
                &self,
                export: $crate::__private::api::SessionExport,
                spawner: $crate::__private::api::guatiao::library::Object<
                    dyn $crate::__private::api::ProcessSpawner,
                >,
            ) -> ::core::result::Result<
                $crate::__private::api::ExportPage,
                $crate::__private::api::guatiao::library::ProviderError,
            > {
                $crate::__private::api::AcpHarness::export_session(&self.inner, export, spawner)
            }
        }

        $(#[$accounts_meta])*
        #[derive($crate::__private::api::guatiao::Provider, ::core::fmt::Debug)]
        #[provider(
            $crate::__private::api::AccountProvider,
            id = $accounts_id,
            config = $account_settings,
            new = $accounts::unconfigured,
            crate = $crate::__private::api::guatiao
        )]
        pub struct $accounts {
            inner: $crate::stdio::StdioAccounts,
        }

        impl ::core::convert::From<$account_settings> for $accounts {
            fn from(settings: $account_settings) -> Self {
                Self {
                    inner: $crate::stdio::StdioAccounts::new(&$def, settings.into()),
                }
            }
        }

        impl $accounts {
            /// A provider configured as `settings` say.
            #[must_use]
            pub fn new(settings: $account_settings) -> Self {
                settings.into()
            }

            fn unconfigured() -> Self {
                Self::new(<$account_settings as ::core::default::Default>::default())
            }
        }

        impl $crate::__private::api::AccountProvider for $accounts {
            fn describe(
                &self,
            ) -> ::core::result::Result<
                $crate::__private::api::AccountProviderInfo,
                $crate::__private::api::guatiao::library::ProviderError,
            > {
                $crate::__private::api::AccountProvider::describe(&self.inner)
            }

            fn offers(
                &self,
            ) -> ::core::result::Result<
                $crate::__private::api::AccountOffer,
                $crate::__private::api::guatiao::library::ProviderError,
            > {
                $crate::__private::api::AccountProvider::offers(&self.inner)
            }

            fn login(
                &self,
                request: $crate::__private::api::LoginRequest,
                spawner: $crate::__private::api::guatiao::library::Object<
                    dyn $crate::__private::api::ProcessSpawner,
                >,
                prompter: $crate::__private::api::guatiao::library::Object<
                    dyn $crate::__private::api::LoginPrompter,
                >,
            ) -> ::core::result::Result<
                $crate::__private::api::LoginOutcome,
                $crate::__private::api::guatiao::library::ProviderError,
            > {
                $crate::__private::api::AccountProvider::login(&self.inner, request, spawner, prompter)
            }

            fn renew(
                &self,
                request: $crate::__private::api::RenewRequest,
                spawner: $crate::__private::api::guatiao::library::Object<
                    dyn $crate::__private::api::ProcessSpawner,
                >,
            ) -> ::core::result::Result<
                $crate::__private::api::RenewReport,
                $crate::__private::api::guatiao::library::ProviderError,
            > {
                $crate::__private::api::AccountProvider::renew(&self.inner, request, spawner)
            }

            fn watch(
                &self,
                request: $crate::__private::api::WatchRequest,
                spawner: $crate::__private::api::guatiao::library::Object<
                    dyn $crate::__private::api::ProcessSpawner,
                >,
                keeper: $crate::__private::api::guatiao::library::Object<
                    dyn $crate::__private::api::AccountKeeper,
                >,
            ) -> ::core::result::Result<
                $crate::__private::api::guatiao::library::Object<
                    dyn $crate::__private::api::AccountWatch,
                >,
                $crate::__private::api::guatiao::library::ProviderError,
            > {
                $crate::__private::api::AccountProvider::watch(&self.inner, request, spawner, keeper)
            }

            fn provides(
                &self,
                account: $crate::__private::api::AccountRef,
            ) -> ::core::result::Result<
                $crate::__private::alloc::vec::Vec<$crate::__private::alloc::string::String>,
                $crate::__private::api::guatiao::library::ProviderError,
            > {
                $crate::__private::api::AccountProvider::provides(&self.inner, account)
            }

            fn signed_in(
                &self,
                query: $crate::__private::api::SignedInQuery,
            ) -> ::core::result::Result<
                $crate::__private::api::SignedInCheck,
                $crate::__private::api::guatiao::library::ProviderError,
            > {
                $crate::__private::api::AccountProvider::signed_in(&self.inner, query)
            }

            fn usage(
                &self,
                query: $crate::__private::api::AccountUsageQuery,
                spawner: $crate::__private::api::guatiao::library::Object<
                    dyn $crate::__private::api::ProcessSpawner,
                >,
                keeper: $crate::__private::api::guatiao::library::Object<
                    dyn $crate::__private::api::AccountKeeper,
                >,
            ) -> ::core::result::Result<
                $crate::__private::api::AccountUsage,
                $crate::__private::api::guatiao::library::ProviderError,
            > {
                $crate::__private::api::AccountProvider::usage(&self.inner, query, spawner, keeper)
            }

            fn models(
                &self,
                query: $crate::__private::api::AccountModelsQuery,
                spawner: $crate::__private::api::guatiao::library::Object<
                    dyn $crate::__private::api::ProcessSpawner,
                >,
                keeper: $crate::__private::api::guatiao::library::Object<
                    dyn $crate::__private::api::AccountKeeper,
                >,
            ) -> ::core::result::Result<
                $crate::__private::api::ModelListing,
                $crate::__private::api::guatiao::library::ProviderError,
            > {
                $crate::__private::api::AccountProvider::models(&self.inner, query, spawner, keeper)
            }
        }
    };
}
