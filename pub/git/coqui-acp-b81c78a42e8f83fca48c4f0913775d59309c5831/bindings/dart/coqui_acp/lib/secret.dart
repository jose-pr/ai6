/// The secret-prompt extension (`docs/secrets.md`): forms whose secret fields travel sealed to a per-request key that is bound to an
/// identity the collector already knows. Identity keys are never handed to this library: a product passes functions that sign and
/// agree on keys through its platform key store or WebCrypto. For raw keys held in memory, in tests and tools, see
/// `package:coqui_acp/secret_unprotected.dart`.
library;

export 'src/secret.dart'
    show
        AcpIdentityId,
        AcpLinkKey,
        AcpLinkRole,
        AcpOpened,
        AcpSealed,
        AcpSecretAccepted,
        AcpSecretAgreement,
        AcpSecretBinding,
        AcpSecretCancelled,
        AcpSecretCollector,
        AcpSecretContext,
        AcpSecretDeclined,
        AcpSecretException,
        AcpSecretFailure,
        AcpSecretField,
        AcpSecretForm,
        AcpSecretLimits,
        AcpSecretMethod,
        AcpSecretProver,
        AcpSecretReceiver,
        AcpSecretRecipient,
        AcpSecretRequestParams,
        AcpSecretRequestReply,
        AcpSecretShowDeliver,
        AcpSecretShowOpen,
        AcpSecretShowOpened,
        AcpSecretSigner,
        AcpSecretValue,
        AcpSecretVerifier,
        AcpSecretsCapability,
        acpBindingDh,
        acpBindingEcdsa,
        acpBindingLink,
        acpSecretDefaultLifetime,
        acpSecretRequestMethod,
        acpSecretShowDeliverMethod,
        acpSecretShowDone,
        acpSecretShowDoneFromJson,
        acpSecretShowOpenMethod,
        acpSecretsCapability,
        acpSecretsVersion;
