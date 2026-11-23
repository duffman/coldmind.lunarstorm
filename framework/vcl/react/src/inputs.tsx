/**
 * The display and input controls of spec 2.4: label, image, progressbar, button,
 * edit, memo, checkbox, radiogroup, combobox and trackbar.
 *
 * Decision: every input reports its event with the property values the user
 * changed (`onChange` with `text`, `onClick` with `checked`, …), whether or not
 * the view resource connects a handler. The backend's control proxy has to hold
 * the current value either way; the backend decides whether a handler runs.
 */
import type { ReactElement } from 'react';
import { frame, useControl, type CommonProps } from './control';

/** A line of text. */
export function Label(literal: CommonProps): ReactElement {
  const { props, emit } = useControl(literal);
  return <span {...frame('label', props, emit)}>{props.caption}</span>;
}

/** Props of `<image>`. */
export interface ImageProps extends CommonProps {
  /** The image's URL, relative to the asset origin. */
  src?: string;
  /** Scales the image to the control's size. */
  stretch?: boolean;
  /** Keeps the aspect ratio when stretching. */
  proportional?: boolean;
}

/** A picture. Its caption is the alternative text. */
export function Image(literal: ImageProps): ReactElement {
  const { props, emit } = useControl(literal);
  const fit = !props.stretch ? 'none' : props.proportional ? 'contain' : 'fill';
  const attributes = frame('image', props, emit);
  return <img {...attributes} src={props.src} alt={props.caption ?? ''} style={{ ...attributes.style, objectFit: fit }} />;
}

/** Props of the controls that hold a position in a range. */
export interface RangeProps extends CommonProps {
  /** The current position. */
  position?: number;
  /** The lowest position; 0 when absent. */
  min?: number;
  /** The highest position; 100 when absent. */
  max?: number;
}

/** A progress indicator. */
export function ProgressBar(literal: RangeProps): ReactElement {
  const { props, emit } = useControl(literal);
  const min = props.min ?? 0;
  const max = props.max ?? 100;
  return <progress {...frame('progressbar', props, emit)} value={(props.position ?? min) - min} max={max - min} aria-label={props.caption} />;
}

/** Props of `<button>`. */
export interface ButtonProps extends CommonProps {
  /** Clicked by Enter anywhere on the form. */
  default?: boolean;
  /** Clicked by Escape anywhere on the form. */
  cancel?: boolean;
  /** Name of the action the button stands for; resolved by the backend. */
  action?: string;
}

/** A push button; reports `onClick`. */
export function Button(literal: ButtonProps): ReactElement {
  const { props, emit } = useControl(literal);
  return (
    <button {...frame('button', props, emit, props.default && 'vcl-button-default')} type="button" disabled={props.enabled === false}
            tabIndex={props.tabOrder} data-default={props.default || undefined} data-cancel={props.cancel || undefined}
            onClick={() => emit('onClick')}>
      {props.caption}
    </button>
  );
}

/** Props of `<edit>`. */
export interface EditProps extends CommonProps {
  /** The text. */
  text?: string;
  /** Shown while the text is empty. */
  placeholder?: string;
  /** The user cannot change the text. */
  readOnly?: boolean;
  /** Hides the characters. */
  password?: boolean;
}

/** A single-line text field; reports `onChange` with `text`. */
export function Edit(literal: EditProps): ReactElement {
  const { props, emit } = useControl(literal);
  return (
    <input {...frame('edit', props, emit)} type={props.password ? 'password' : 'text'} value={props.text ?? ''}
           placeholder={props.placeholder} readOnly={props.readOnly} disabled={props.enabled === false} tabIndex={props.tabOrder}
           aria-label={props.caption} onChange={event => emit('onChange', { text: event.target.value })} />
  );
}

/** Props of `<memo>`. */
export interface MemoProps extends EditProps {
  /** Wraps long lines; on when absent. */
  wordWrap?: boolean;
}

/** A multi-line text field; reports `onChange` with `text`. */
export function Memo(literal: MemoProps): ReactElement {
  const { props, emit } = useControl(literal);
  return (
    <textarea {...frame('memo', props, emit)} value={props.text ?? ''} placeholder={props.placeholder} readOnly={props.readOnly}
              disabled={props.enabled === false} tabIndex={props.tabOrder} wrap={props.wordWrap === false ? 'off' : 'soft'}
              aria-label={props.caption} onChange={event => emit('onChange', { text: event.target.value })} />
  );
}

/** Props of `<checkbox>`. */
export interface CheckBoxProps extends CommonProps {
  /** Whether the box is ticked. */
  checked?: boolean;
}

/** A check box with its caption; reports `onClick` with `checked`. */
export function CheckBox(literal: CheckBoxProps): ReactElement {
  const { props, emit } = useControl(literal);
  return (
    <label {...frame('checkbox', props, emit)}>
      <input type="checkbox" checked={props.checked ?? false} disabled={props.enabled === false} tabIndex={props.tabOrder}
             onChange={event => emit('onClick', { checked: event.target.checked })} />
      <span>{props.caption}</span>
    </label>
  );
}

/** Props of the controls that pick one of a list of strings. */
export interface ItemsProps extends CommonProps {
  /** The choices. */
  items?: readonly string[];
  /** Index of the chosen item; -1 for none. */
  itemIndex?: number;
}

/** A titled group of radio buttons; reports `onClick` with `itemIndex`. */
export function RadioGroup(literal: ItemsProps): ReactElement {
  const { props, emit } = useControl(literal);
  return (
    <fieldset {...frame('radiogroup', props, emit)} disabled={props.enabled === false} role="radiogroup">
      {props.caption ? <legend>{props.caption}</legend> : null}
      {(props.items ?? []).map((item, index) => (
        <label key={index} className="vcl-radio">
          <input type="radio" name={props.name} checked={index === props.itemIndex}
                 onChange={() => emit('onClick', { itemIndex: index })} />
          <span>{item}</span>
        </label>
      ))}
    </fieldset>
  );
}

/** A drop-down list; reports `onChange` with `itemIndex`. */
export function ComboBox(literal: ItemsProps): ReactElement {
  const { props, emit } = useControl(literal);
  return (
    <select {...frame('combobox', props, emit)} value={props.itemIndex ?? -1} disabled={props.enabled === false}
            tabIndex={props.tabOrder} aria-label={props.caption}
            onChange={event => emit('onChange', { itemIndex: Number(event.target.value) })}>
      {/* The entry for "nothing chosen"; a <select> cannot show an empty selection otherwise. */}
      <option value={-1} hidden />
      {(props.items ?? []).map((item, index) => (
        <option key={index} value={index}>{item}</option>
      ))}
    </select>
  );
}

/** A slider; reports `onChange` with `position`. */
export function TrackBar(literal: RangeProps): ReactElement {
  const { props, emit } = useControl(literal);
  const min = props.min ?? 0;
  return (
    <input {...frame('trackbar', props, emit)} type="range" min={min} max={props.max ?? 100} value={props.position ?? min}
           disabled={props.enabled === false} tabIndex={props.tabOrder} aria-label={props.caption}
           onChange={event => emit('onChange', { position: Number(event.target.value) })} />
  );
}
