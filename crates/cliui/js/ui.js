/**
 * `lunarstorm:ui` - terminal styling, progress bars and spinners.
 *
 *     import { style, progress, spinner } from 'lunarstorm:ui';
 *
 * Design decisions:
 * - Bars and spinners draw on stderr, so a script's stdout stays clean for pipes.
 * - All live items share one region at the bottom of the terminal. Each line of it
 *   ends in a newline and the cursor rests below it, so anything else that prints
 *   (an uncaught error, the shell prompt) starts on a clean line.
 * - Redraws are throttled to one per FRAME_MS. `update()` redraws on its own when a
 *   frame is due, so bars advance inside synchronous loops too; a timer redraws
 *   while the script is waiting on I/O, which is what animates spinners.
 * - `console.*` output while items are live is printed above the region (the
 *   region is erased, the line printed, the region redrawn).
 * - When stderr is not a terminal nothing is animated: each item prints exactly one
 *   line when it finishes, without escape sequences unless color is forced.
 * - The 16-color tier and NO_COLOR terminals get block glyphs for the bar, because
 *   filled and empty cells cannot be told apart by color there.
 */

const { ops, live } = __lunarstorm.internal;

/** Milliseconds between redraws of the live region. */
const FRAME_MS = 50;
/** Milliseconds each spinner frame is shown. */
const SPIN_MS = 80;
/** Spinner animation frames. */
const FRAMES = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
/** Bar gradient start (violet) and end (cyan), matching the CLI brand. */
const FROM = [167, 139, 250];
const TO = [34, 211, 238];
/** Widest and narrowest bar, in cells. */
const MAX_BAR = 40;
const MIN_BAR = 10;

/**
 * Converts a color to the SGR foreground parameter a color depth supports.
 * @param {number} depth 1 basic, 2 256 colors, 3 truecolor
 * @param {number} r red, 0-255
 * @param {number} g green, 0-255
 * @param {number} b blue, 0-255
 * @return {string} SGR parameter
 */
function fg(depth, r, g, b) {
  if (depth === 3) return '38;2;' + r + ';' + g + ';' + b;
  if (depth === 2) {
    const cube = (c) => Math.round((c / 255) * 5);
    return '38;5;' + (16 + 36 * cube(r) + 6 * cube(g) + cube(b));
  }
  const max = Math.max(r, g, b) * 0.75;
  return String(90 + (r > max ? 1 : 0) + (g > max ? 2 : 0) + (b > max ? 4 : 0));
}

/**
 * Builds the styling functions for one stream.
 * @param {number} depth the stream's color depth; 0 makes every function a no-op
 * @return {object} the style object
 */
function makeStyle(depth) {
  const wrap = (code) => (text) => (depth ? '\x1b[' + code + 'm' + text + '\x1b[0m' : String(text));
  return {
    /** Whether styling is active on this stream. */
    enabled: depth > 0,
    bold: wrap('1'),
    dim: wrap('2'),
    italic: wrap('3'),
    underline: wrap('4'),
    red: wrap('91'),
    green: wrap('92'),
    yellow: wrap('93'),
    blue: wrap('94'),
    magenta: wrap('95'),
    cyan: wrap('96'),
    gray: wrap('90'),
    /**
     * Colors text with a 24-bit color, degraded to what the terminal supports.
     * @param {number} r red, 0-255
     * @param {number} g green, 0-255
     * @param {number} b blue, 0-255
     * @return {function(string): string}
     */
    rgb: (r, g, b) => wrap(fg(depth, r, g, b)),
    /**
     * Colors text with a left-to-right gradient.
     * @param {string} text text
     * @param {number[]} [from] start color `[r, g, b]`; violet by default
     * @param {number[]} [to] end color `[r, g, b]`; cyan by default
     * @return {string}
     */
    gradient(text, from = FROM, to = TO) {
      const chars = Array.from(String(text));
      if (depth < 2) return wrap(fg(depth, ...from))(chars.join(''));
      const last = Math.max(chars.length - 1, 1);
      let out = '';
      for (let i = 0; i < chars.length; i++) {
        const mix = (k) => Math.round(from[k] + ((to[k] - from[k]) * i) / last);
        out += '\x1b[' + fg(depth, mix(0), mix(1), mix(2)) + 'm' + chars[i];
      }
      return out + '\x1b[0m';
    },
  };
}

/** Styles for text the script prints to stdout (`console.log`). */
export const style = makeStyle(ops.color[0]);
/** Styles for the live region and finish lines, which go to stderr. */
const ink = makeStyle(ops.color[1]);
/** Whether stderr is a terminal, i.e. whether anything animates. */
const animated = ops.tty[1];
/** Whether the bar can use color to separate filled from empty cells. */
const fine = ops.color[1] >= 2;

/** Items currently in the live region, top to bottom. */
const items = [];
/** Lines of the region currently on screen. */
let drawn = 0;
/** Interval that redraws while the script is idle; `null` when nothing is live. */
let timer = null;
/** `ops.now()` of the last redraw. */
let painted = 0;

/** Erases the live region, leaving the cursor where its first line was. */
function erase() {
  if (drawn) ops.print(2, '\x1b[' + drawn + 'A\x1b[J');
  drawn = 0;
}

/** Draws every live item, overwriting the previous frame in place. */
function paint() {
  const cols = ops.columns();
  let out = drawn ? '\x1b[' + drawn + 'A' : '';
  for (const item of items) out += '\r\x1b[2K' + item.render(cols) + '\n';
  ops.print(2, out);
  drawn = items.length;
  painted = ops.now();
}

/** Redraws if a frame is due. */
function refresh() {
  if (timer !== null && ops.now() - painted >= FRAME_MS) paint();
}

/**
 * Adds an item to the live region, starting live mode for the first one.
 * @param {{render: function(number): string}} item the item
 */
function attach(item) {
  if (!animated) return;
  items.push(item);
  if (timer === null) {
    ops.live(true);
    live.wrap = (fd, text) => {
      erase();
      ops.print(fd, text);
      ops.flush();
      paint();
    };
    timer = setInterval(paint, FRAME_MS);
  }
  paint();
}

/**
 * Removes an item and prints its final line above what remains of the region.
 * @param {object} item the item
 * @param {?string} line the permanent line to leave behind; `null` for none
 */
function detach(item, line) {
  const at = items.indexOf(item);
  if (at >= 0) items.splice(at, 1);
  erase();
  if (line !== null) ops.print(2, line + '\n');
  if (items.length) return paint();
  if (timer !== null) {
    clearInterval(timer);
    timer = null;
    live.wrap = null;
    ops.live(false);
  }
}

/**
 * @param {number} n a byte count
 * @return {string} e.g. `6.2 MB`
 */
function bytes(n) {
  const units = ['B', 'kB', 'MB', 'GB', 'TB'];
  let u = 0;
  while (n >= 1000 && u < units.length - 1) {
    n /= 1000;
    u++;
  }
  return (u ? n.toFixed(1) : String(Math.round(n))) + ' ' + units[u];
}

/**
 * @param {number} seconds a duration
 * @return {string} e.g. `0.4s`, `12s`, `3m07s`
 */
function duration(seconds) {
  if (seconds < 10) return seconds.toFixed(1) + 's';
  if (seconds < 60) return Math.round(seconds) + 's';
  const whole = Math.round(seconds);
  return Math.floor(whole / 60) + 'm' + String(whole % 60).padStart(2, '0') + 's';
}

/**
 * Draws the bar itself.
 * @param {number} fraction progress, 0 to 1
 * @param {number} width cells
 * @return {string}
 */
function bar(fraction, width) {
  const exact = fraction * width;
  const full = Math.floor(exact);
  if (!fine) return '█'.repeat(full) + '░'.repeat(width - full);
  const half = full < width && exact - full >= 0.5;
  const rest = width - full - (half ? 1 : 0);
  // The gradient spans the whole track, so the visible part keeps its colors as it grows.
  const track = Array.from(ink.gradient('━'.repeat(width)).matchAll(/\x1b\[[\d;]+m━/g), (m) => m[0]);
  return track.slice(0, full).join('') + (half ? track[full].replace('━', '╸') : '') + '\x1b[0m' + ink.gray('━'.repeat(rest));
}

/**
 * Starts a progress bar.
 * @param {object} [options] settings
 * @param {number} [options.total] value at which the bar is complete; default 100
 * @param {string} [options.label] text shown before the bar
 * @param {number} [options.width] bar width in cells; fitted to the terminal by default
 * @param {boolean} [options.bytes] show values and rate as byte sizes
 * @return {{value: number, update: function(number): void, tick: function(number=): void,
 *   done: function(string=): void, fail: function(string=): void}} the bar
 */
export function progress({ total = 100, label = '', width = 0, bytes: asBytes = false } = {}) {
  const started = ops.now();
  const show = asBytes ? bytes : (n) => String(Math.round(n));
  let open = true;
  const self = {
    /** Current value. */
    value: 0,
    /**
     * @param {number} cols terminal width
     * @return {string} one line of the live region
     */
    render(cols) {
      const fraction = total > 0 ? Math.min(self.value / total, 1) : 0;
      const elapsed = (ops.now() - started) / 1000;
      const rate = elapsed > 0 ? self.value / elapsed : 0;
      const eta = rate > 0 ? duration((total - self.value) / rate) : '--';
      const percent = String(Math.round(fraction * 100)).padStart(3) + '%';
      const stats = show(self.value) + '/' + show(total) + '  ' + show(rate) + '/s  eta ' + eta;
      const cells = width || Math.max(MIN_BAR, Math.min(MAX_BAR, cols - label.length - stats.length - 10));
      return (label ? ink.bold(label) + ' ' : '') + bar(fraction, cells) + ' ' + ink.bold(percent) + '  ' + ink.dim(stats);
    },
    /**
     * Sets the value. Reaching `total` completes the bar.
     * @param {number} value new value
     */
    update(value) {
      if (!open) return;
      self.value = Math.min(value, total);
      if (self.value >= total) self.done();
      else refresh();
    },
    /**
     * Advances the value.
     * @param {number} [n] amount; default 1
     */
    tick(n = 1) {
      self.update(self.value + n);
    },
    /**
     * Completes the bar and leaves a summary line.
     * @param {string} [message] text for the line; the label by default
     */
    done(message = label) {
      finish(ink.green('✔'), message);
    },
    /**
     * Stops the bar as failed and leaves a summary line.
     * @param {string} [message] text for the line; the label by default
     */
    fail(message = label) {
      finish(ink.red('✖'), message);
    },
  };
  /**
   * Closes the bar once.
   * @param {string} mark status glyph
   * @param {string} message summary text
   */
  function finish(mark, message) {
    if (!open) return;
    open = false;
    const summary = show(self.value) + ' in ' + duration((ops.now() - started) / 1000);
    detach(self, mark + ' ' + (message ? message + '  ' : '') + ink.dim(summary));
  }
  attach(self);
  return self;
}

/**
 * Starts a spinner for work of unknown length.
 * @param {string} [label] text shown after the spinner
 * @return {{text: function(string): void, succeed: function(string=): void,
 *   fail: function(string=): void, stop: function(): void}} the spinner
 */
export function spinner(label = '') {
  const started = ops.now();
  let open = true;
  const self = {
    /**
     * @return {string} one line of the live region
     */
    render() {
      const frame = FRAMES[Math.floor((ops.now() - started) / SPIN_MS) % FRAMES.length];
      return ink.rgb(...TO)(frame) + ' ' + label + ' ' + ink.dim(duration((ops.now() - started) / 1000));
    },
    /**
     * Changes the label.
     * @param {string} next new label
     */
    text(next) {
      label = next;
      refresh();
    },
    /**
     * Stops the spinner with a success mark.
     * @param {string} [message] text for the line; the label by default
     */
    succeed(message = label) {
      finish(ink.green('✔') + ' ' + message);
    },
    /**
     * Stops the spinner with a failure mark.
     * @param {string} [message] text for the line; the label by default
     */
    fail(message = label) {
      finish(ink.red('✖') + ' ' + message);
    },
    /** Stops the spinner and leaves no line behind. */
    stop() {
      if (!open) return;
      open = false;
      detach(self, null);
    },
  };
  /**
   * Closes the spinner once.
   * @param {string} line the permanent line
   */
  function finish(line) {
    if (!open) return;
    open = false;
    detach(self, line + ' ' + ink.dim(duration((ops.now() - started) / 1000)));
  }
  attach(self);
  return self;
}

export default { style, progress, spinner };
