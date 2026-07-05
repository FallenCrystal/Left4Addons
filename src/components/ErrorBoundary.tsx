import { Component, ErrorInfo, ReactNode } from 'react';
import { AlertTriangle, RefreshCw } from 'lucide-react';

interface Props {
  children?: ReactNode;
}

interface State {
  hasError: boolean;
  error: Error | null;
  errorInfo: ErrorInfo | null;
}

export class ErrorBoundary extends Component<Props, State> {
  public state: State = {
    hasError: false,
    error: null,
    errorInfo: null
  };

  public static getDerivedStateFromError(error: Error): State {
    return { hasError: true, error, errorInfo: null };
  }

  public componentDidCatch(error: Error, errorInfo: ErrorInfo) {
    console.error('Uncaught error:', error, errorInfo);
    this.setState({ error, errorInfo });
  }

  private handleReset = () => {
    this.setState({ hasError: false, error: null, errorInfo: null });
  };

  private handleReload = () => {
    window.location.reload();
  };

  public render() {
    if (this.state.hasError) {
      return (
        <div style={{
          display: 'flex',
          flexDirection: 'column',
          alignItems: 'center',
          justifyContent: 'center',
          height: '100vh',
          width: '100vw',
          backgroundColor: 'var(--md-sys-color-background)',
          color: 'var(--md-sys-color-on-background)',
          padding: '40px',
          boxSizing: 'border-box'
        }}>
          <AlertTriangle size={64} style={{ color: 'var(--md-sys-color-error)', marginBottom: '24px' }} />
          <h1 style={{ marginTop: 0, marginBottom: '16px', fontSize: '28px', color: 'var(--md-sys-color-error)' }}>
            应用遇到意外错误 (Frontend Crash)
          </h1>
          
          <div style={{
            backgroundColor: 'var(--md-sys-surface-container-high)',
            border: '1px solid var(--md-sys-color-outline-variant)',
            borderRadius: '16px',
            padding: '24px',
            maxWidth: '800px',
            width: '100%',
            overflow: 'auto',
            marginBottom: '32px',
            textAlign: 'left'
          }}>
            <h3 style={{ margin: '0 0 12px 0', color: 'var(--md-sys-color-primary)' }}>Error: {this.state.error?.toString()}</h3>
            <pre style={{ 
              margin: 0, 
              color: 'var(--md-sys-color-on-surface-variant)', 
              fontSize: '13px', 
              lineHeight: '1.5',
              whiteSpace: 'pre-wrap',
              wordBreak: 'break-all'
            }}>
              {this.state.errorInfo?.componentStack}
            </pre>
          </div>

          <div style={{ display: 'flex', gap: '16px' }}>
            <button className="btn btn-secondary" onClick={this.handleReset}>
              尝试恢复界面
            </button>
            <button className="btn btn-primary" onClick={this.handleReload}>
              <RefreshCw size={16} />
              <span>重新加载应用</span>
            </button>
          </div>
        </div>
      );
    }

    return this.props.children;
  }
}
