import type { Control } from './controls';
import type { Form } from './form';

/** The native-window boundary. A Tao adapter would implement this contract. */
export interface DesktopHost {
  open(form: Form): void;
  update(control: Control): void;
  close(form: Form): void;
  dispose(): void;
}
