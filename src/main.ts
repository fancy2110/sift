import { mount } from 'svelte';
import './app.css';
import App from './App.svelte';
import { startupMark, observeFirstPaint } from './lib/startup';
import { syncWindowTheme } from './lib/window-theme';

startupMark('webview:main-module');
observeFirstPaint();

// Match the native title bar to the content color scheme as early as possible.
void syncWindowTheme();

const app = mount(App, { target: document.getElementById('app')! });
startupMark('webview:app-mounted');

export default app;
