//! HandPointer library + sign-in bootstrap, evaluated in the webview as
//! an `initialization_script` so it runs on every navigation before the
//! page's own scripts.
//!
//! The HandPointer library attaches itself to `globalThis.psyops.HandPointer`.
//! The bootstrap looks for a Sign-in link matching the x.ai pattern and
//! shows a pointer at it; on pages without the link it hides any
//! previously-shown pointer.

const HAND_POINTER_JS: &str = include_str!("../resources/hand_pointer.js");

const SIGN_IN_BOOTSTRAP: &str = r#"
(() => {
  if (!globalThis.psyops || !globalThis.psyops.HandPointer) return;
  if (globalThis.psyops._xAppPointer) {
    globalThis.psyops._xAppPointer.hide();
    globalThis.psyops._xAppPointer = null;
  }
  const link = document.querySelector('a[href*="accounts.x.ai/sign-in"]');
  if (!link) return;
  globalThis.psyops._xAppPointer =
      globalThis.psyops.HandPointer.create({
        target: link,
        direction: 'left',
        text: 'Sign in to your X developer account',
      });
  globalThis.psyops._xAppPointer.show();
})();
"#;

/// Concatenated initialization script: library defines
/// `psyops.HandPointer`, then the bootstrap calls into it.
pub fn init_script() -> String {
    format!("{}\n{}", HAND_POINTER_JS, SIGN_IN_BOOTSTRAP)
}
