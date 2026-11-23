import { Application, BrowserHost } from '../desktop';
import { MainForm } from './forms/main-form';
import { ProjectStore } from './models/project-store';
import './styles.css';

const root = document.getElementById('app');
if (!root) throw new Error('The application requires an #app host element.');

const application = new Application(new BrowserHost(root));
application.run(new MainForm(application, new ProjectStore()));
