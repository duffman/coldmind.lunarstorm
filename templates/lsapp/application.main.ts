import { Application, BrowserHost } from './common';
import { MyDashView } from './tooling/generated-views';
import './views/MyDash/MyDash.css';

const root = document.getElementById('app');
if (!root) throw new Error('The desktop host requires an application mount point.');

const application = new Application(new BrowserHost(root));
application.run(MyDashView.view, MyDashView.layout);
