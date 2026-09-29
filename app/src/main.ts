import { mount } from 'svelte';
import App from './App.svelte';
import './app.css';
import 'uplot/dist/uPlot.min.css';

export default mount(App, { target: document.getElementById('app')! });
