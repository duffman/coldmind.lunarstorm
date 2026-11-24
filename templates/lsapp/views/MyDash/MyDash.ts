import {
  Button, Inject, Label, StatusBar, View,
  ViewAction, ViewClass, ViewOutlet,
} from '../../common';
import { CounterService } from '../../services/CounterService';

/** Persistent view object; its paired .tsx file supplies the control tree. */
@ViewClass
export class MyDash extends View {
  title = 'Counter';
  width = 680;
  height = 480;

  @ViewOutlet(Button)
  declare incrementButton: Button;

  @ViewOutlet(Button)
  declare resetButton: Button;

  @ViewOutlet(Label)
  declare countLabel: Label;

  @ViewOutlet(StatusBar)
  declare status: StatusBar;

  @Inject(CounterService)
  declare private counter: CounterService;

  protected onCreate(): void {
    this.showCount();
    this.status.text = 'Ready';
  }

  @ViewAction
  increment(): void {
    this.counter.increment();
    this.showCount();
    this.status.text = 'Counter increased';
  }

  @ViewAction
  reset(): void {
    this.counter.reset();
    this.showCount();
    this.status.text = 'Counter reset';
  }

  private showCount(): void {
    this.countLabel.caption = String(this.counter.value);
    this.resetButton.enabled = this.counter.value !== 0;
  }
}
