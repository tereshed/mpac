import { createRoot } from 'react-dom/client'
import './style.css'
import App from './App'
import './select-controls.css'
import MobileClient from './MobileClient'
import { mobileClient } from './native-mobile'
import './workspace-density.css'
import './touch-workspace.css'

if (mobileClient()) document.documentElement.classList.add('native-mobile-client')
createRoot(document.getElementById('root')!).render(mobileClient() ? <MobileClient /> : <App />)
