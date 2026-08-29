function decodeBase64Url(value: string): ArrayBuffer {
  const base64 = value
    .replace(/-/g, "+")
    .replace(/_/g, "/")
    .padEnd(Math.ceil(value.length / 4) * 4, "=");
  return Uint8Array.from(atob(base64), (char) => char.charCodeAt(0)).buffer;
}

function encodeBase64Url(value: ArrayBuffer): string {
  const bytes = new Uint8Array(value);
  let binary = "";
  bytes.forEach((byte) => (binary += String.fromCharCode(byte)));
  return btoa(binary)
    .replace(/\+/g, "-")
    .replace(/\//g, "_")
    .replace(/=+$/, "");
}

export type CredentialDescriptorJson = Record<string, unknown> & { id: string };
export type RegistrationOptionsJson = Record<string, unknown> & {
  challenge: string;
  user: Record<string, unknown> & { id: string };
  excludeCredentials?: CredentialDescriptorJson[];
};
export type AuthenticationOptionsJson = Record<string, unknown> & {
  challenge: string;
  allowCredentials?: CredentialDescriptorJson[];
};
export type WebAuthnEnvelope<T> = { token: string; public_key: { publicKey: T } };

export function registrationOptions(
  value: RegistrationOptionsJson,
): PublicKeyCredentialCreationOptions {
  return {
    ...value,
    challenge: decodeBase64Url(value.challenge),
    user: { ...value.user, id: decodeBase64Url(value.user.id) },
    excludeCredentials: value.excludeCredentials?.map((item) => ({
      ...item,
      id: decodeBase64Url(item.id),
    })),
  } as PublicKeyCredentialCreationOptions;
}

export function authenticationOptions(
  value: AuthenticationOptionsJson,
): PublicKeyCredentialRequestOptions {
  return {
    ...value,
    challenge: decodeBase64Url(value.challenge),
    allowCredentials: value.allowCredentials?.map((item) => ({
      ...item,
      id: decodeBase64Url(item.id),
    })),
  } as PublicKeyCredentialRequestOptions;
}

export function credentialJson(
  credential: PublicKeyCredential,
): Record<string, unknown> {
  const response = credential.response;
  if (response instanceof AuthenticatorAttestationResponse) {
    return {
      id: credential.id,
      rawId: encodeBase64Url(credential.rawId),
      type: credential.type,
      response: {
        attestationObject: encodeBase64Url(response.attestationObject),
        clientDataJSON: encodeBase64Url(response.clientDataJSON),
        transports: response.getTransports?.(),
      },
    };
  }
  const assertion = response as AuthenticatorAssertionResponse;
  return {
    id: credential.id,
    rawId: encodeBase64Url(credential.rawId),
    type: credential.type,
    response: {
      authenticatorData: encodeBase64Url(assertion.authenticatorData),
      clientDataJSON: encodeBase64Url(assertion.clientDataJSON),
      signature: encodeBase64Url(assertion.signature),
      userHandle: assertion.userHandle
        ? encodeBase64Url(assertion.userHandle)
        : null,
    },
  };
}
