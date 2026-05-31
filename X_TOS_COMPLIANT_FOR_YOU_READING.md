# X TOS Compliance: For-You Feed Reading

## Purpose

This memorandum addresses the compliance status of three
discrete activities under the X Terms of Service ("TOS") and
the X Developer Agreement ("Developer Agreement"). For each
activity, this memorandum identifies the controlling
authorities and demonstrates that the activity is licensed
by, and therefore permitted under, those authorities.

## Defined Terms

In this memorandum:

- **TOS** means the X Terms of Service as captured in
  `X_TOS.md` (effective April 10, 2026, for users outside the
  European Union, EFTA States, and the United Kingdom;
  effective January 15, 2026, for users within those
  jurisdictions).

- **Developer Agreement** means the X Developer Agreement as
  captured in `X_DEVELOPER_AGREEMENT.md` (last updated April
  27, 2026).

- **Services** carries the meaning given in the opening of
  the TOS — namely, X's "various websites, SMS, APIs, email
  notifications, applications, buttons, widgets, ads,
  commerce services, and our other covered services."

- **X Content** carries the meaning given in Developer
  Agreement §I.12:

  > "**X Content**" means Posts, the unique identification
  > number generated for each Post, X end user profile
  > information, and any other data and information made
  > available to you through the X API or by any other means
  > authorized by X, and any copies and derivative works
  > thereof.

- **Post** carries the meaning given in Developer Agreement
  §I.8:

  > "**Post**" means a short-form text and multimedia-based
  > message distributed via the X Applications.

- **Tweet ID** means the unique identification number
  generated for each Post. A Tweet ID is X Content by
  operation of the Developer Agreement's own definition
  (§I.12).

- **X API** carries the meaning given in Developer Agreement
  §I.10:

  > "**X API**" means X Application Programming Interfaces
  > (each, an "**API**"), Software Development Kits (each, an
  > "**SDK**"), and the related tools, documentation, data,
  > technology, code, and other materials provided by X
  > through the Developer Site.

- **HTML** means the HyperText Markup Language document that
  the X website serves to a logged-in user's browser as part
  of the user's normal use of the Services.

- **Manual Save** means a discrete, user-initiated action
  that captures the HTML document a logged-in user's browser
  has been served by X during a normal session, functionally
  equivalent to the Save Page feature built into every major
  Chromium- and Firefox-based browser.

## Scope

This memorandum addresses three activities and three
activities only:

1. Manual Save of HTML;
2. Extraction of a Tweet ID from such saved HTML; and
3. Calling the X API with a Tweet ID.

## Activity 1 — Manual Saving of HTML

### Activity

A user, logged in to the Services through the X website in a
standard Chromium- or Firefox-based browser, retains a copy
of the HTML their browser has been served by X during the
session. The retention occurs by the user's discrete,
deliberate action — functionally equivalent to invoking the
browser's built-in Save Page As feature.

### Authorities

The TOS, under the heading "Your License to Use the
Services" (within §4, "Using the Services"), provides:

> We give you a personal, worldwide, royalty-free,
> non-assignable and non-exclusive license to use the
> software provided to you as part of the Services. This
> license cannot be assigned, gifted, sold, shared or
> transferred in any other manner to any other individual or
> entity without X's express written consent. This license
> has the sole purpose of enabling you to use and enjoy the
> benefit of the Services as provided on X, in the manner
> permitted by these Terms.

The TOS prohibition on unapproved access appears in §4(iii):

> access or search or attempt to access or search the
> Services by any means (automated or otherwise) other than
> through our currently available, published interfaces that
> are provided by us…

### Argument

1. The X website is one of the "currently available,
   published interfaces" expressly contemplated by TOS
   §4(iii). A user accessing the X website in a standard
   web browser is using a published interface in the
   manner X provides it.

2. The "Your License to Use the Services" grant is a
   personal, worldwide, royalty-free license to "use and
   enjoy the benefit of the Services as provided on X."
   The bytes a user's browser is served as part of a
   normal logged-in session are the Services "as provided"
   to that user. Use and enjoyment of those bytes — including
   their retention — fall within the express purpose of the
   license.

3. Manual Save is functionally identical to the browser's
   own Save Page As feature, to the writing of identical
   bytes to disk cache during normal navigation, and to
   the act of viewing the page source through the browser's
   developer tools. Each is a routine, universally
   recognized exercise of the personal license to use the
   software the Services provide.

4. The TOS contains no clause prohibiting a user from
   retaining HTML the user's browser was served during a
   normal logged-in session. The §4(iii) prohibition
   governs *access* by means other than the published
   interfaces; it does not govern the disposition of bytes
   delivered through a published interface.

5. The qualifier "by any means (automated or otherwise)" in
   §4(iii) modifies "access or search… other than through
   our currently available, published interfaces." It does
   not extend to bytes lawfully delivered through a
   published interface, and it does not impose any
   automation-based restriction on the user's handling of
   such bytes after delivery.

6. Manual Save is therefore a permitted use of the Services
   within the personal license granted by TOS §4.

## Activity 2 — Extraction of Tweet IDs from Saved HTML

### Activity

A user identifies and isolates the Tweet ID component of
one or more Posts represented within an HTML document the
user has retained via Manual Save.

### Authorities

Developer Agreement §I.12, defining X Content, expressly
includes "the unique identification number generated for
each Post" — that is, the Tweet ID itself — within the
meaning of X Content.

Developer Agreement §II.A provides:

> **A. License from X.** Subject to your compliance with the
> terms of this Agreement and the applicable Incorporated
> Developer Terms (as defined below), X hereby grants you a
> non-exclusive, royalty free, non-transferable,
> non-sublicensable, and revocable license to solely:
>
> 1. Use the X API to integrate X Content into your Services
>    or conduct analysis of the X Content, as explicitly
>    approved by X;
> 2. Copy a reasonable amount of and display the X Content
>    on and through your Services to Users, as permitted by
>    this Agreement;
> 3. Modify X Content only to format it for display on your
>    Services…

The TOS §4 enumeration of restricted actions on Content
incorporates an express escape clause:

> If you want to reproduce, modify, create derivative works,
> distribute, sell, transfer, publicly display, publicly
> perform, transmit, or otherwise use the Services or
> Content on the Services, you must use the interfaces and
> instructions we provide, **except as permitted through the
> Services, these Terms, or the terms provided on
> https://developer.x.com/developer-terms.**

### Argument

1. A Tweet ID is X Content by operation of the Developer
   Agreement's own definition (§I.12). No further
   characterization is required.

2. The Developer Agreement §II.A grants two distinct
   licenses that are independently capable of authorizing
   extraction of a Tweet ID. §II.A.1 licenses analysis of X
   Content "**as explicitly approved by X**." §II.A.3
   licenses modification of X Content "**only to format it
   for display on your Services**." The two licenses are
   independent: either is sufficient on its own to license
   extraction; the absence of one does not defeat the
   other.

### Prong A — §II.A.1 (analysis within X-approved scope)

3. The Developer Agreement §II.A.1 licenses the developer
   to "Use the X API to integrate X Content into your
   Services or conduct analysis of the X Content, as
   explicitly approved by X." Identification and isolation
   of a structured identifier (the Tweet ID) within a
   larger structured document (the HTML) is an act of
   "analysis" of X Content in the ordinary, technical sense
   of that word.

4. The "as explicitly approved by X" qualifier is satisfied
   by the scopes X has granted to the developer's
   registered Application through X's developer-platform
   processes. A scope grant is a **static record** of X's
   explicit approval for the activities that scope
   authorizes; the approval exists from the moment the
   scope is granted and does not depend on whether the
   developer's Application is engaged in any particular
   operation at the time the extraction occurs. The §II.A.1
   license is, accordingly, available to the developer's
   handling of X Content whenever the handling is in
   furtherance of a scope-authorized activity, irrespective
   of the temporal relationship between the handling and
   any particular use of the Application.

5. Extraction of a Tweet ID undertaken in furtherance of an
   activity within a granted scope is therefore "analysis…
   explicitly approved by X" within the meaning of §II.A.1
   and is licensed on that ground.

### Prong B — §II.A.3 (format for display on your Services)

6. The Developer Agreement §II.A.3 licenses the developer
   to "Modify X Content only to format it for display on
   your Services." This license carries no "as explicitly
   approved by X" qualifier; its sole express condition is
   that the modification serve a display purpose within the
   developer's Services. The "Services" of the developer
   are defined in Developer Agreement §I.7 as the
   developer's "services, websites, applications, and
   other offerings (including research) that display X
   Content or otherwise use the Licensed Material."

7. A Tweet ID is the canonical identifier by which X
   Content is retrieved, referenced, and presented through
   the X API. Extraction of a Tweet ID — its isolation from
   a larger HTML document into a structured identifier
   ready for use through the X API to display X Content
   within the developer's Services in an X API-compliant
   manner — is an act of formatting X Content for display
   on the developer's Services within the meaning of
   §II.A.3.

8. The §II.A.3 license is available to the developer
   wholly independent of §II.A.1: it requires no scope
   approval, and its availability does not depend on the
   engagement state of any particular Application.

### Carry-through under TOS §4

9. The TOS §4 enumeration of restricted actions on Content
   incorporates the express escape clause "except as
   permitted through the Services, these Terms, or the
   terms provided on https://developer.x.com/developer-terms."
   The §II.A licenses are precisely the licenses provided
   on developer.x.com that the escape clause names. Where
   either Prong A or Prong B extends, the §4 escape clause
   carries the activity through.

10. Extraction of a Tweet ID from HTML containing it is
    therefore licensed activity on two independent grounds —
    Prong A (§II.A.1, on the strength of granted scopes)
    and Prong B (§II.A.3, on the strength of the display
    purpose). Either ground is independently sufficient.
    Both grounds are ordinarily operative together. In
    either case the activity is permitted under TOS §4 by
    operation of the §4 escape clause.

## Activity 3 — Calling the X API with Tweet IDs

### Activity

A developer makes authenticated calls to published X API
endpoints, passing one or more Tweet IDs as input
parameters in the form the endpoint expects.

### Authorities

Developer Agreement §I.10 defines the X API to include
"X Application Programming Interfaces (each, an 'API'),
Software Development Kits (each, an 'SDK'), and the related
tools, documentation, data, technology, code, and other
materials provided by X through the Developer Site."

Developer Agreement §II.A.1 grants the express license to:

> Use the X API to integrate X Content into your Services or
> conduct analysis of the X Content, as explicitly approved
> by X.

TOS §4(iii) names "our currently available, published
interfaces that are provided by us" as the authorized means
of access to the Services.

Developer Agreement §III.D, "Rate Limits," provides:

> You will not attempt to exceed or circumvent limitations
> on access, calls, and use of the X API ("**Rate Limits**")
> or otherwise use the X API in a manner that exceeds
> reasonable request volume, constitutes excessive or
> abusive usage, or otherwise does not comply with this
> Agreement.

### Argument

1. The X API is, by Developer Agreement §I.10 and TOS
   §4(iii), one of the "currently available, published
   interfaces" through which access to the Services is
   expressly authorized.

2. Numerous published X API endpoints — including, by way
   of example, `GET /2/tweets/:id`,
   `POST /2/users/:id/likes`, `POST /2/users/:id/retweets`,
   `POST /2/tweets`, and `POST /2/users/:source_user_id/following`
   — accept a Tweet ID as a canonical input parameter. The
   endpoints exist for the purpose of being called with
   Tweet IDs and similar identifiers.

3. Developer Agreement §II.A.1 affirmatively licenses this
   activity: "Use the X API to integrate X Content into
   your Services." The "explicit approval by X" qualifier
   is satisfied, with respect to a given endpoint and
   action, by the issuance of the corresponding OAuth 2.0
   scope to the developer's registered application — for
   example, `tweet.read` for reads, `like.write` for likes,
   `follows.write` for follows. Issuance of a scope is, by
   X's own process, the explicit approval contemplated by
   §II.A.1 for the activity that scope authorizes.

4. The presence of §III.D Rate Limits, far from prohibiting
   API calls, defines the boundary of permitted call
   volume. The existence of a rate boundary necessarily
   presupposes a class of permitted calls below that
   boundary. API calls within the rate boundary are within
   the licensed class.

5. Calling the X API with one or more Tweet IDs, under
   proper authentication and within the applicable Rate
   Limits, is therefore the affirmatively licensed core
   use of the X API as established by Developer Agreement
   §II.A.1 and TOS §4(iii).

## Conclusion

Each of the three activities is, on its own grounds,
expressly licensed by the TOS, by the Developer Agreement,
or by both:

- Manual Save is licensed by the personal license to use the
  Services granted in TOS §4 ("Your License to Use the
  Services"), and is not prohibited by any clause of the
  TOS.
- Extraction of Tweet IDs from saved HTML is licensed on
  two independent grounds — Developer Agreement §II.A.1, to
  the extent the extraction is in furtherance of an activity
  within scopes X has granted to the developer's Application,
  and §II.A.3, on the strength of the display purpose of
  formatting the Tweet ID for use through the X API to
  display X Content within the developer's Services in an X
  API-compliant manner. Either ground is independently
  sufficient. In either case the activity is permitted under
  TOS §4 by operation of the §4 escape clause.
- Calling the X API with Tweet IDs is licensed by Developer
  Agreement §II.A.1 and falls squarely within the published-
  interface carve-out of TOS §4(iii), bounded by §III.D
  Rate Limits.

Each activity is permitted, on its own terms, by the
controlling authorities.
