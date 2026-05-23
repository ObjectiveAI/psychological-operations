// HandPointer overlay library — vanilla JS, attaches to globalThis.psyops.HandPointer.
//
// Lifted verbatim from the (now-dormant) chromium-fork's
// components/psyops/hand_pointer.cc raw string literal. Evaluated as an
// initialization_script in Tauri's webview at every navigation.

(() => {
  const HAND = { left: '\u{1F449}', right: '\u{1F448}' };
  const HOST_ATTR = 'data-psyops-hand-pointer';

  class HandPointer {
    static create(opts) { return new HandPointer(opts); }

    constructor({ target, direction = 'left', text = '' } = {}) {
      this.target = target;
      this.direction = direction;
      this.text = text;
      this.host = null;
      this.shadow = null;
      this._onReflow = () => this._position();
    }

    show() {
      if (this.host) return this;
      this.host = document.createElement('div');
      this.host.setAttribute(HOST_ATTR, '');
      Object.assign(this.host.style, {
        position: 'fixed', top: '0', left: '0',
        pointerEvents: 'none', zIndex: '2147483646',
      });
      this.shadow = this.host.attachShadow({ mode: 'closed' });
      this.shadow.innerHTML = `
        <style>
          :host { all: initial; }
          .root { position: absolute; display: inline-flex;
                  flex-direction: column; align-items: center;
                  font-family: system-ui, -apple-system, sans-serif; }
          .hand { font-size: 48px; line-height: 1; user-select: none; }
          .text { margin-top: 4px; padding: 6px 10px;
                  background: rgba(0,0,0,0.85); color: #fff;
                  font-size: 13px; border-radius: 6px;
                  max-width: 240px; text-align: center; }
          .text:empty { display: none; }
        </style>
        <div class="root">
          <span class="hand"></span>
          <span class="text"></span>
        </div>`;
      this._render();
      document.body.appendChild(this.host);
      window.addEventListener('resize', this._onReflow, true);
      window.addEventListener('scroll', this._onReflow, true);
      this._position();
      return this;
    }

    hide() {
      if (!this.host) return this;
      window.removeEventListener('resize', this._onReflow, true);
      window.removeEventListener('scroll', this._onReflow, true);
      this.host.remove();
      this.host = this.shadow = null;
      return this;
    }

    update(opts = {}) {
      if ('target' in opts) this.target = opts.target;
      if ('direction' in opts) this.direction = opts.direction;
      if ('text' in opts) this.text = opts.text;
      if (this.host) { this._render(); this._position(); }
      return this;
    }

    _render() {
      this.shadow.querySelector('.hand').textContent =
        HAND[this.direction] ?? HAND.left;
      this.shadow.querySelector('.text').textContent = this.text || '';
    }

    _position() {
      const rect = this._rect();
      if (!rect) return;
      const handSize = 48, gap = 8;
      const root = this.shadow.querySelector('.root');
      const x = this.direction === 'left'
        ? rect.left - handSize - gap
        : rect.right + gap;
      const y = rect.top + (rect.height - handSize) / 2;
      root.style.left = `${x}px`;
      root.style.top = `${y}px`;
    }

    _rect() {
      if (!this.target) return null;
      if (this.target instanceof Element) {
        return this.target.getBoundingClientRect();
      }
      const { x = 0, y = 0, width = 0, height = 0 } = this.target;
      return { left: x, top: y, right: x + width, bottom: y + height,
               width, height };
    }
  }

  const ns = (globalThis.psyops = globalThis.psyops || {});
  ns.HandPointer = HandPointer;
})();
