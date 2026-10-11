import { mount } from 'svelte';
import './app.css';
import App from './App.svelte';
import { startupMark, observeFirstPaint } from './lib/startup';

startupMark('webview:main-module');
observeFirstPaint();

const app = mount(App, { target: document.getElementById('app')! });
startupMark('webview:app-mounted');

export default app;
