import net from "node:net";

const code = `(async () => {
  const out = { url: location.href };

  // Identity
  out.userAgent = navigator.userAgent;
  out.uaData = navigator.userAgentData
    ? {
        brands: navigator.userAgentData.brands,
        mobile: navigator.userAgentData.mobile,
        platform: navigator.userAgentData.platform,
      }
    : null;
  try {
    out.uaHighEntropy = navigator.userAgentData
      ? await navigator.userAgentData.getHighEntropyValues([
          "architecture", "bitness", "model", "platform",
          "platformVersion", "uaFullVersion", "fullVersionList", "wow64",
        ])
      : null;
  } catch (e) { out.uaHighEntropy = "err: " + String(e); }

  // Common bot tells
  out.webdriver = navigator.webdriver;
  out.languages = navigator.languages;
  out.language = navigator.language;
  out.platform = navigator.platform;
  out.vendor = navigator.vendor;
  out.product = navigator.product;
  out.productSub = navigator.productSub;
  out.hardwareConcurrency = navigator.hardwareConcurrency;
  out.deviceMemory = navigator.deviceMemory;
  out.maxTouchPoints = navigator.maxTouchPoints;
  out.cookieEnabled = navigator.cookieEnabled;
  out.doNotTrack = navigator.doNotTrack;

  // Plugins / mime types (real Chrome has 3+ each; headless has 0)
  out.pluginsCount = navigator.plugins.length;
  out.pluginsNames = [...navigator.plugins].map(p => p.name);
  out.mimeTypesCount = navigator.mimeTypes.length;

  // window.chrome (present in Chrome, ABSENT or stripped in WebView2)
  out.windowChromeType = typeof window.chrome;
  out.chromeRuntimeType = window.chrome ? typeof window.chrome.runtime : "n/a";
  out.chromeAppType = window.chrome ? typeof window.chrome.app : "n/a";

  // Screen / window
  out.screen = { w: screen.width, h: screen.height, cw: screen.availWidth, ch: screen.availHeight, depth: screen.colorDepth };
  out.viewport = { w: innerWidth, h: innerHeight, dpr: devicePixelRatio };

  // WebGL renderer (often "Google SwiftShader" or "ANGLE..." for WebView2)
  try {
    const c = document.createElement("canvas").getContext("webgl");
    const ext = c?.getExtension("WEBGL_debug_renderer_info");
    out.webgl = {
      vendor: c?.getParameter(c.VENDOR),
      renderer: c?.getParameter(c.RENDERER),
      unmaskedVendor: ext ? c.getParameter(ext.UNMASKED_VENDOR_WEBGL) : null,
      unmaskedRenderer: ext ? c.getParameter(ext.UNMASKED_RENDERER_WEBGL) : null,
    };
  } catch (e) { out.webgl = "err: " + String(e); }

  // Permissions weirdness (headless returns 'denied' for notifications)
  try {
    const p = await navigator.permissions.query({ name: "notifications" });
    out.permissionsNotifications = { state: p.state, notificationPermission: Notification.permission };
  } catch (e) { out.permissionsNotifications = "err: " + String(e); }

  // Feature detection
  out.features = {
    webRTC: !!window.RTCPeerConnection,
    serviceWorker: !!navigator.serviceWorker,
    webAuthn: !!window.PublicKeyCredential,
    notification: !!window.Notification,
    push: !!window.PushManager,
    bluetooth: !!navigator.bluetooth,
    usb: !!navigator.usb,
    serial: !!navigator.serial,
  };

  // Misc tells
  out.toString = window.toString === undefined ? "n/a" : window.toString.toString();
  out.outerWidth_eq_innerWidth = outerWidth === innerWidth;

  return out;
})()`;

const msg = JSON.stringify({ type: "eval", code }) + "\n";
const client = net.connect("\\\\.\\pipe\\psyops_browser_stdin");
client.on("connect", () => client.end(msg));
client.on("error", (e) => { console.error(e.message); process.exit(1); });
