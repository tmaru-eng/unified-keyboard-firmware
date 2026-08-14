import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { App } from './App';
import { MemoryBridgeDevice, SerialDeviceTransport } from './device';

createRoot(document.getElementById('root')!).render(<StrictMode><App device={new SerialDeviceTransport(new MemoryBridgeDevice())} /></StrictMode>);
