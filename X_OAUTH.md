**Yes, as of February 2026 (and still current), the X API uses a pay-per-use / pay-per-usage (credit-based) pricing model with no subscriptions or monthly caps.** You purchase credits upfront in the official Developer Console (console.x.com), and credits are deducted in real-time per API request/resource. Exact per-endpoint rates (e.g., reads vs. writes) are shown live in the console and can change, but actions like liking or retweeting typically fall under "User Interaction: Create" costing around $0.015 per request (confirm current pricing in your console).

Billing/credits are **tied to a single developer account** (the X account you use to sign into the Developer Console). Usage from **all apps, projects, and authenticated users** under that developer account draws from the **same shared credit pool**. There is no per-app or per-user separate billing—everything aggregates at the developer-account level. You can monitor usage/credits live in the console, set spending limits or auto-recharge, and even earn bonus xAI API credits based on your spend.

### How to Set This Up for Your 5 X Accounts (Shared Billing)
You do **not** need 5 separate developer accounts or apps. One developer account + one app is sufficient and gives you fully shared credits:

1. Choose (or create) **one main X account** to serve as your developer account. Sign into the [X Developer Platform](https://developer.x.com/) / Developer Console with it.

2. In the Developer Console, create **one App** (or a Project containing an app). This gives you the app's API keys/credentials.

3. For each of your 5 X accounts (including the developer account if desired):
   - Use **OAuth 2.0 (User Context)** or OAuth 1.0a to authorize the app for that account.
   - This generates an access token (and refresh token) specific to each user/account.
   - Standard flow: "Sign in with X" / 3-legged OAuth (or PIN-based for bots). You only need to do this once per account (tokens can be stored securely).

4. When making API calls to **like** (e.g., `POST /2/users/{user_id}/likes`) or **retweet** (e.g., via retweet or quote endpoints) on behalf of a specific account:
   - Use the **app's consumer keys** + the **specific user's access token** for that account.
   - All requests (regardless of which of the 5 accounts is acting) will deduct from the **single shared credit balance** in your developer account's console.

This is the standard, documented way to manage multiple X accounts with one app/developer setup. It has worked for years (pre- and post-pay-per-use) for bots, dashboards, or multi-account tools.

**Key caveats / things to watch:**
- **Rate limits**: Still exist (per user token and sometimes app-level). They are separate from the credit billing.
- **Platform rules**: Automation of likes/retweets must comply with X's spam, manipulation, and automation policies (e.g., no bulk spam-like behavior across accounts). Violations can lead to app suspension or account bans.
- **Exact costs**: Likes/retweets are write/interaction operations—check the live rates in your Developer Console, as they are per-request and subject to change.
- **Legacy plans**: If you had an old Basic/Pro plan, you can migrate to pay-per-use, but new setups default to this model.

This setup keeps everything simple, centralized, and fully shared under one credit pool. If you run into console-specific details, the official docs and console dashboard are the best real-time source. Let me know if you need code snippets for the OAuth flow or specific endpoints!