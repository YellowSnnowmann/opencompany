// External links surfaced in the console. Mirrors OpenHuman's canonical URLs.

export const DISCORD_INVITE_URL = "https://discord.tinyhumans.ai";

/**
 * Where a TinyHumans API key is created. One constant because two surfaces
 * send an operator there — the setup wizard's Managed key field and the
 * Account page's Connect to TinyHumans dialog — and they must not point at
 * two different pages.
 */
export const TINYHUMANS_API_KEYS_URL = "https://tinyhumans.ai/dashboard?tab=api-keys";
