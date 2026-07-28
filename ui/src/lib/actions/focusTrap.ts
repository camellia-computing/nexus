export interface FocusTrapOptions {
  onEscape?: () => void;
  initialFocus?: string;
}

const focusableSelector = [
  'a[href]',
  'button:not([disabled])',
  'input:not([disabled])',
  'select:not([disabled])',
  'textarea:not([disabled])',
  '[tabindex]:not([tabindex="-1"])',
].join(',');

const activeTraps: HTMLElement[] = [];

function focusableElements(node: HTMLElement): HTMLElement[] {
  return Array.from(node.querySelectorAll<HTMLElement>(focusableSelector)).filter((element) => {
    const style = getComputedStyle(element);
    return !element.hidden && style.display !== 'none' && style.visibility !== 'hidden';
  });
}

export function focusTrap(node: HTMLElement, initialOptions: FocusTrapOptions = {}) {
  let options = initialOptions;
  const previousFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;

  function focusInitial() {
    const requested = options.initialFocus
      ? node.querySelector<HTMLElement>(options.initialFocus)
      : null;
    (requested ?? focusableElements(node)[0] ?? node).focus({ preventScroll: true });
  }

  function handleKeydown(event: KeyboardEvent) {
    if (activeTraps[activeTraps.length - 1] !== node || node.closest('[inert]')) return;
    if (event.key === 'Escape' && options.onEscape) {
      event.preventDefault();
      event.stopPropagation();
      options.onEscape();
      return;
    }
    if (event.key !== 'Tab') return;

    const elements = focusableElements(node);
    if (!elements.length) {
      event.preventDefault();
      node.focus({ preventScroll: true });
      return;
    }

    const first = elements[0];
    const last = elements[elements.length - 1];
    const active = document.activeElement;
    if (!node.contains(active)) {
      event.preventDefault();
      (event.shiftKey ? last : first).focus();
    } else if (event.shiftKey && active === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && active === last) {
      event.preventDefault();
      first.focus();
    }
  }

  if (!node.hasAttribute('tabindex')) node.tabIndex = -1;
  activeTraps.push(node);
  document.addEventListener('keydown', handleKeydown);
  queueMicrotask(focusInitial);

  return {
    update(nextOptions: FocusTrapOptions = {}) {
      options = nextOptions;
    },
    destroy() {
      document.removeEventListener('keydown', handleKeydown);
      const index = activeTraps.lastIndexOf(node);
      if (index >= 0) activeTraps.splice(index, 1);
      if (previousFocus?.isConnected) previousFocus.focus({ preventScroll: true });
    },
  };
}
