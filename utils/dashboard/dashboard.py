#!/usr/bin/env python3
"""
Interactive Dashboard for Rusty-Comms Benchmark Results

A comprehensive Plotly Dash application for visualizing IPC benchmark results
from both JSON and CSV output formats with interactive filtering and analysis.
"""

import argparse
import json
import logging
import traceback
import pandas as pd
import plotly.express as px
import plotly.graph_objects as go
from plotly.subplots import make_subplots
import dash
from dash import dcc, html, Input, Output, State, callback, dash_table, ALL
from dash.exceptions import PreventUpdate
import numpy as np
import threading
import time
from concurrent.futures import ThreadPoolExecutor, as_completed

from scipy import stats
try:
    from sklearn.ensemble import IsolationForest
    SKLEARN_AVAILABLE = True
except ImportError:
    SKLEARN_AVAILABLE = False
from pathlib import Path
from typing import Dict, List, Tuple, Optional, Union
import os
import weakref

# Import modular components
from cache import dashboard_cache, cached_computation, safe_computation
from data_processor import BenchmarkDataProcessor, list_directories, get_directory_breadcrumbs

# Configure logging
logging.basicConfig(level=logging.INFO, format='%(asctime)s - %(name)s - %(levelname)s - %(message)s')
logger = logging.getLogger(__name__)

class DashboardApp:
    """Main Dash application for interactive benchmark visualization."""
    
    def __init__(self, data_store: Dict, initial_dir: str = "."):
        self.data_store = data_store
        self.initial_dir = initial_dir
        self.app = dash.Dash(__name__)
        
        # Load external CSS file
        import os
        css_file_path = os.path.join(os.path.dirname(__file__), 'dashboard_styles.css')
        
        # Inject modern CSS
        self.app.index_string = '''
        <!DOCTYPE html>
        <html>
            <head>
                {%metas%}
                <title>{%title%}</title>
                {%favicon%}
                {%css%}
                <link rel="stylesheet" type="text/css" href="/static/dashboard_styles.css">
            </head>
            <body>
                {%app_entry%}
                <footer>
                    {%config%}
                    {%scripts%}
                    {%renderer%}
                </footer>
            </body>
        </html>
        '''
        
        # Serve the CSS file as a static asset
        @self.app.server.route('/static/dashboard_styles.css')
        def serve_css():
            with open(css_file_path, 'r') as f:
                css_content = f.read()
            from flask import Response
            return Response(css_content, mimetype='text/css')
        self.setup_layout()
        self.setup_callbacks()
    
    def apply_dark_theme(self, fig):
        """Apply enhanced dark theme matching panel backgrounds with neon colors for maximum visibility."""
        fig.update_layout(
            # Match panel background colors for consistency
            paper_bgcolor='#21262d',
            plot_bgcolor='#21262d',
            
            # Font styling with brighter text
            font=dict(color='#ffffff', family='Inter, sans-serif'),
            
            # Grid and axes styling with subtle gray for structure
            xaxis=dict(
                gridcolor='#333333',
                linecolor='#666666',
                tickcolor='#666666',
                color='#ffffff',
                showgrid=True,
                gridwidth=1
            ),
            yaxis=dict(
                gridcolor='#333333',
                linecolor='#666666',
                tickcolor='#666666',
                color='#ffffff',
                showgrid=True,
                gridwidth=1
            ),
            
            # Legend styling matching panel background
            legend=dict(
                bgcolor='rgba(33, 38, 45, 0.9)',
                bordercolor='#666666',
                borderwidth=1,
                font=dict(color='#ffffff')
            ),
            
            # Hover styling
            hoverlabel=dict(
                bgcolor='#21262d',
                bordercolor='#666666',
                font_color='#ffffff'
            )
        )
        
        # Update all traces for dark theme
        for trace in fig.data:
            if hasattr(trace, 'marker') and trace.marker:
                if hasattr(trace.marker, 'line'):
                    trace.marker.line.color = '#30363d'
        
        return fig
    
    def reload_data(self, directory_path: str) -> tuple:
        """Reload data from a new directory and update the data store."""
        try:
            from pathlib import Path
            
            # Validate directory path
            dir_path = Path(directory_path)
            if not dir_path.exists():
                return False, f"Error: Directory '{directory_path}' does not exist."
            
            if not dir_path.is_dir():
                return False, f"Error: '{directory_path}' is not a directory."
            
            # Load data from new directory
            logger.info(f"Reloading data from directory: {directory_path}")
            data_loader = BenchmarkDataProcessor(dir_path)
            new_data_store = data_loader.process_all_data()
            
            # Check if any data was found
            summary_count = len(new_data_store['summary_data'])
            streaming_count = len(new_data_store['streaming_data'])
            
            if summary_count == 0 and streaming_count == 0:
                return False, f"Warning: No benchmark data files found in '{directory_path}'."
            
            # Update the data store
            self.data_store.update(new_data_store)
            
            # Success message
            success_msg = f"Success: Loaded {summary_count} summary records and {streaming_count:,} streaming records from '{directory_path}'."
            logger.info(success_msg)
            
            return True, success_msg
            
        except Exception as e:
            error_msg = f"Error loading data: {str(e)}"
            logger.error(error_msg)
            return False, error_msg
    
    def get_filter_options(self):
        """Extract unique values for filter controls."""
        summary_df = self.data_store['summary_data']
        streaming_df = self.data_store['streaming_data']
        
        # Combine mechanisms from both datasets
        mechanisms = set()
        message_sizes = set()
        
        if not summary_df.empty:
            mechanisms.update(summary_df['mechanism'].unique())
            # Filter out 0 and invalid message sizes
            valid_message_sizes = [x for x in summary_df['message_size'].unique() if x > 0]
            message_sizes.update(valid_message_sizes)
        
        if not streaming_df.empty:
            mechanisms.update(streaming_df['mechanism'].unique())
            # Filter out 0 and invalid message sizes
            valid_message_sizes = [x for x in streaming_df['message_size'].unique() if x > 0]
            message_sizes.update(valid_message_sizes)
        
        result = {
            'mechanisms': sorted(list(mechanisms)),
            'message_sizes': sorted(list(message_sizes))
        }
        
        logger.info(f"Filter options: {result}")
        return result
    
    def setup_layout(self):
        """Setup the main dashboard layout."""
        filter_options = self.get_filter_options()
        
        self.app.layout = html.Div(children=[
            html.Div([
                # Sidebar Controls
                html.Div([
                    html.H1("Rusty-Comms Dashboard", className="header-title"),
                    
                    # Analysis Control Section - MOVED TO TOP
                    html.Div([
                        html.H3("Analysis", style={'margin-bottom': '10px'}),
                        html.P("Run analysis with current filters to generate charts and insights.", 
                               style={'color': '#6b7280', 'font-size': '0.9rem', 'margin-bottom': '15px', 'font-style': 'italic'}),
                        
                        html.Button(
                            [
                                html.Span(id='run-analysis-button-text', children='Run Analysis')
                            ],
                            id='run-analysis-button',
                            n_clicks=0,
                            style={
                                'width': '100%',
                                'padding': '12px 20px',
                                'background': 'linear-gradient(135deg, #1f6feb 0%, #0969da 100%)',
                                'color': 'white',
                                'border': 'none',
                                'border-radius': '6px',
                                'cursor': 'pointer',
                                'font-size': '1rem',
                                'font-weight': '700',
                                'margin-bottom': '10px',
                                'box-shadow': '0 2px 8px rgba(31, 111, 235, 0.3)',
                                'transition': 'all 0.2s ease'
                            }
                        ),
                        
                        html.Button(
                            ['Clear Cache'],
                            id='clear-cache-button',
                            n_clicks=0,
                            style={
                                'width': '100%',
                                'padding': '8px 16px',
                                'background': 'linear-gradient(135deg, #6c7293 0%, #57606a 100%)',
                                'color': 'white',
                                'border': 'none',
                                'border-radius': '6px',
                                'cursor': 'pointer',
                                'font-size': '0.9rem',
                                'font-weight': '500',
                                'transition': 'all 0.2s ease'
                            }
                        ),
                        
                        # Status display for analysis
                        html.Div(id='analysis-status', style={'margin-top': '10px'})
                    ], className="filter-section"),
                    
                    # Data Directory Controls with File Browser
                    html.Div([
                        html.H3("Data Directory"),
                        html.P("Browse and select a directory containing benchmark data files.", 
                               style={'color': '#6b7280', 'font-size': '0.9rem', 'margin-bottom': '10px', 'font-style': 'italic'}),
                        
                        # Current selected directory display
                        html.Div([
                            html.P(
                                id='current-directory-display',
                                children=f"{self.initial_dir}",
                                style={
                                    'background': 'linear-gradient(135deg, #21262d 0%, #161b22 100%)',
                                    'padding': '8px 12px',
                                    'border-radius': '6px',
                                    'border': '1px solid #30363d',
                                    'font-family': 'monospace',
                                    'font-size': '0.85rem',
                                    'margin-bottom': '10px',
                                    'word-break': 'break-all',
                                    'color': '#f0f6fc'
                                }
                            ),
                            
                            html.Button(
                                ['Browse Directory...'],
                                id='browse-directory-button',
                                n_clicks=0,
                                style={
                                    'width': '100%',
                                    'padding': '10px 16px',
                                    'background': 'linear-gradient(135deg, #2da44e 0%, #1a7f37 100%)',
                                    'color': 'white',
                                    'border': 'none',
                                    'border-radius': '6px',
                                    'cursor': 'pointer',
                                    'font-size': '0.95rem',
                                    'font-weight': '600',
                                    'margin-bottom': '10px',
                                    'transition': 'all 0.2s ease',
                                    'box-shadow': '0 2px 8px rgba(45, 164, 78, 0.3)'
                                }
                            ),
                            
                            html.Button(
                                ['Reload Data'],
                                id='reload-data-button',
                                n_clicks=0,
                                style={
                                    'width': '100%',
                                    'padding': '10px 16px',
                                    'background': 'linear-gradient(135deg, #1f6feb 0%, #0969da 100%)',
                                    'color': 'white',
                                    'border': 'none',
                                    'border-radius': '6px',
                                    'cursor': 'pointer',
                                    'font-size': '0.95rem',
                                    'font-weight': '600',
                                    'transition': 'all 0.2s ease',
                                    'box-shadow': '0 2px 8px rgba(31, 111, 235, 0.3)'
                                }
                            ),
                        ], style={'margin-bottom': '20px'}),
                        
                        # Status message for reload feedback
                        html.Div(id='reload-status', style={'margin-bottom': '15px'})
                    ], className="filter-section"),
                    
                    html.Div([
                        html.H3("Filters"),
                        html.P("Select items to filter data. All options are selected by default.", 
                               style={'color': '#6b7280', 'font-size': '0.9rem', 'margin-bottom': '15px', 'font-style': 'italic'}),
                        
                        html.Label("Mechanisms:"),
                        dcc.Checklist(
                            id='mechanism-filter',
                            options=[{'label': m, 'value': m} for m in filter_options['mechanisms']],
                            value=filter_options['mechanisms'],  # Start with all mechanisms selected
                            className="filter-checklist"
                        ),
                    ], className="filter-section"),
                    
                    html.Div([
                        html.Label("Message Sizes:"),
                        dcc.Checklist(
                            id='message-size-filter',
                            options=[{'label': f"{s}B", 'value': s} for s in filter_options['message_sizes']],
                            value=filter_options['message_sizes'],  # Start with all message sizes selected
                            className="filter-checklist"
                        ),
                    ], className="filter-section"),
                    
                ], className="sidebar"),
                
                # Main Content
                html.Div([
                    dcc.Tabs(
                        id="main-tabs", 
                        value="summary-tab",
                        style={
                            'height': '50px',
                            'marginBottom': '25px',
                            'backgroundColor': '#21262d',
                            'borderBottom': '1px solid #30363d'
                        },
                        children=[
                            dcc.Tab(
                                label="Summary", 
                                value="summary-tab",
                                style={
                                    'padding': '12px 24px', 
                                    'fontWeight': '500',
                                    'backgroundColor': '#21262d',
                                    'color': '#8b949e',
                                    'border': '1px solid #30363d',
                                    'borderBottom': 'none'
                                },
                                selected_style={
                                    'padding': '12px 24px', 
                                    'fontWeight': '600', 
                                    'color': '#f0f6fc',
                                    'backgroundColor': '#0d1117',
                                    'border': '1px solid #58a6ff',
                                    'borderBottom': '1px solid #0d1117'
                                }
                            ),
                            dcc.Tab(
                                label="Time Series", 
                                value="timeseries-tab",
                                style={
                                    'padding': '12px 24px', 
                                    'fontWeight': '500',
                                    'backgroundColor': '#21262d',
                                    'color': '#8b949e',
                                    'border': '1px solid #30363d',
                                    'borderBottom': 'none'
                                },
                                selected_style={
                                    'padding': '12px 24px', 
                                    'fontWeight': '600', 
                                    'color': '#f0f6fc',
                                    'backgroundColor': '#0d1117',
                                    'border': '1px solid #58a6ff',
                                    'borderBottom': '1px solid #0d1117'
                                }
                            ),

                        ]
                    ),
                    html.Div(id="tab-content", className="tab-content")
                ], className="main-content")
                
            ], className="dashboard-container"),
            
            # File Browser Modal
            html.Div([
                html.Div([
                    html.Div([
                        # Modal Header
                        html.Div([
                            html.H3("Browse Directory", style={'margin': 0, 'color': '#f0f6fc'}),
                            html.Button(
                                "×",
                                id="file-browser-close",
                                n_clicks=0,
                                style={
                                    'background': 'none',
                                    'border': 'none',
                                    'font-size': '24px',
                                    'cursor': 'pointer',
                                    'color': '#6b7280'
                                }
                            )
                        ], style={'display': 'flex', 'justify-content': 'space-between', 'align-items': 'center', 'margin-bottom': '20px'}),
                        
                        # Breadcrumb Navigation
                        html.Div(id='directory-breadcrumbs', style={'margin-bottom': '15px'}),
                        
                        # Directory Listing
                        html.Div([
                            html.Div(
                                id='directory-listing',
                                style={
                                    'max-height': '400px',
                                    'overflow-y': 'auto',
                                    'border': '1px solid #30363d',
                                    'border-radius': '8px',
                                    'padding': '10px',
                                    'background': 'linear-gradient(135deg, #21262d 0%, #161b22 100%)'
                                }
                            )
                        ], style={'margin-bottom': '20px'}),
                        
                        # File type legend
                        html.Div([
                            html.P("File Types:", style={'font-size': '12px', 'color': '#6b7280', 'margin': '0 0 5px 0', 'font-weight': 'bold'}),
                            html.Div([
                                html.Span('Folders', style={'font-size': '11px', 'color': '#6b7280', 'margin-right': '15px'}),
                                html.Span('Summary JSON', style={'font-size': '11px', 'color': '#6b7280', 'margin-right': '15px'}),
                                html.Span('Streaming JSON', style={'font-size': '11px', 'color': '#6b7280', 'margin-right': '15px'}),
                                html.Span('CSV Files', style={'font-size': '11px', 'color': '#6b7280'})
                            ])
                        ], style={'margin-bottom': '15px', 'padding': '8px', 'background': 'linear-gradient(135deg, #21262d 0%, #161b22 100%)', 'border-radius': '6px', 'border': '1px solid #30363d'}),
                        
                        # Modal Footer
                        html.Div([
                            html.Button(
                                "Cancel",
                                id="file-browser-cancel",
                                n_clicks=0,
                                style={
                                    'padding': '8px 16px',
                                    'background': 'linear-gradient(135deg, #6c7293 0%, #57606a 100%)',
                                    'color': 'white',
                                    'border': 'none',
                                    'border-radius': '6px',
                                    'cursor': 'pointer',
                                    'margin-right': '10px',
                                    'transition': 'all 0.2s ease'
                                }
                            ),
                            html.Button(
                                "Select Directory",
                                id="file-browser-select",
                                n_clicks=0,
                                style={
                                    'padding': '8px 16px',
                                    'background': 'linear-gradient(135deg, #2da44e 0%, #1a7f37 100%)',
                                    'color': 'white',
                                    'border': 'none',
                                    'border-radius': '6px',
                                    'cursor': 'pointer',
                                    'transition': 'all 0.2s ease',
                                    'box-shadow': '0 2px 8px rgba(45, 164, 78, 0.3)'
                                }
                            )
                        ], style={'display': 'flex', 'justify-content': 'flex-end'})
                        
                    ], style={
                        'background': 'linear-gradient(135deg, #21262d 0%, #161b22 100%)',
                        'padding': '30px',
                        'border-radius': '8px',
                        'box-shadow': '0 16px 64px rgba(0, 0, 0, 0.6)',
                        'border': '1px solid #30363d',
                        'color': '#f0f6fc',
                        'width': '90%',
                        'max-width': '600px',
                        'max-height': '80vh',
                        'overflow-y': 'auto'
                    })
                ], style={
                    'position': 'fixed',
                    'top': 0,
                    'left': 0,
                    'width': '100%',
                    'height': '100%',
                    'background-color': 'rgba(0, 0, 0, 0.5)',
                    'display': 'flex',
                    'justify-content': 'center',
                    'align-items': 'center',
                    'z-index': 1000
                })
            ], id='file-browser-modal', style={'display': 'none'}),
            
            # Hidden components to store file browser state
            dcc.Store(id='current-directory-store', data=self.initial_dir),
            dcc.Store(id='selected-directory-store', data=self.initial_dir)
            
        ], style={'background': '#0d1117', 'minHeight': '100vh', 'color': '#f0f6fc'})
    
    def setup_callbacks(self):
        """Setup all dashboard callbacks."""
        
        # Cache management callbacks
        @self.app.callback(
            Output('analysis-status', 'children'),
            [Input('clear-cache-button', 'n_clicks')],
            prevent_initial_call=True
        )
        def clear_cache(n_clicks):
            if n_clicks > 0:
                dashboard_cache.clear()
                return html.Div([
                    html.Span("Cache cleared", style={'color': '#2da44e', 'font-weight': 'bold'})
                ], style={'padding': '8px', 'background': 'linear-gradient(135deg, #0d4d21 0%, #0a3b1c 100%)', 'border-radius': '4px', 'border': '1px solid #2da44e'})
            return html.Div()

        # Main content callback - now only responds to Run Analysis button
        @self.app.callback(
            Output("tab-content", "children"),
            [Input("run-analysis-button", "n_clicks"),
             Input("main-tabs", "value")],
            [State("mechanism-filter", "value"),
             State("message-size-filter", "value")],
            prevent_initial_call=True
        )
        @safe_computation(default_return=html.Div("Error loading content"))
        def render_tab_content(n_clicks, active_tab, mechanisms, message_sizes):
            if n_clicks == 0:
                return html.Div([
                    html.Div([
                        html.H2("Ready for Analysis", style={'color': '#f0f6fc', 'margin-bottom': '20px'}),
                        html.P("Select your filters and click the 'Run Analysis' button to generate insights and visualizations.", 
                               style={'color': '#8b949e', 'font-size': '1.1rem', 'line-height': '1.6'}),
                        html.Ul([
                            html.Li("Choose mechanisms to analyze (PMQ, SHM, UDS)"),
                            html.Li("Select message sizes for comparison"),
                            html.Li("Click 'Run Analysis' to start processing")
                        ], style={'color': '#f0f6fc', 'padding-left': '20px'})
                    ], className='performance-card', style={'text-align': 'center', 'padding': '60px 40px'})
                ])
            
            try:
                if active_tab == "summary-tab":
                    logger.info(f"Summary tab analysis triggered with filters: mechanisms={mechanisms}, message_sizes={message_sizes}")
                    return self.render_summary_tab_cached(mechanisms, message_sizes)
                elif active_tab == "timeseries-tab":
                    logger.info(f"Time series tab analysis triggered with filters: mechanisms={mechanisms}, message_sizes={message_sizes}")
                    return self.render_timeseries_tab_cached(mechanisms, message_sizes)
                return html.Div("Select a tab")
            except Exception as e:
                logger.error(f"Error rendering tab content: {str(e)}")
                return html.Div([
                    html.H3("Analysis Error", style={'color': '#f85149'}),
                    html.P(f"Error: {str(e)}", style={'color': '#8b949e'})
                ], style={'padding': '20px', 'text-align': 'center'})

        # Simple button status callback 
        @self.app.callback(
            [Output('run-analysis-button-text', 'children'),
             Output('run-analysis-button', 'style')],
            [Input('run-analysis-button', 'n_clicks')],
            [State('run-analysis-button', 'style')],
            prevent_initial_call=True
        )
        def update_analysis_button_status(n_clicks, current_style):
            if n_clicks > 0:
                # Reset to normal state (this will be called after analysis completes)
                reset_style = current_style.copy() if current_style else {
                    'width': '100%',
                    'padding': '12px 20px',
                    'background-color': '#7c2d12',
                    'color': 'white',
                    'border': 'none',
                    'border-radius': '8px',
                    'cursor': 'pointer',
                    'font-size': '1rem',
                    'font-weight': '700',
                    'margin-bottom': '10px',
                    'box-shadow': '0 2px 4px rgba(0, 0, 0, 0.1)'
                }
                reset_style['background'] = 'linear-gradient(135deg, #1f6feb 0%, #0969da 100%)'  # Grafana blue
                return 'Run Analysis', reset_style
            
            return 'Run Analysis', current_style
        


        # Time series charts update callback - only responds to Run Analysis button and tab changes
        @self.app.callback(
            Output('ts-charts-container', 'children'),
            [Input('run-analysis-button', 'n_clicks'),
             Input('main-tabs', 'value')],
            [State('ts-moving-avg-slider', 'value'),
             State('ts-display-points-slider', 'value'),
             State('ts-display-options', 'value'),
             State('ts-chart-layout', 'value'),
             State('ts-view-options', 'value'),
             State('ts-latency-types', 'value'),
             State('ts-statistical-overlays', 'value'),
             State('ts-y-axis-scale', 'value'),
             State('ts-y-axis-range', 'value'),
             State('ts-sampling-strategy', 'value'),
             State("mechanism-filter", "value"),
             State("message-size-filter", "value")],
            prevent_initial_call=True
        )
        @safe_computation(default_return=html.Div("Click 'Run Analysis' to generate time series charts"))
        def update_timeseries_charts(n_clicks, active_tab, moving_avg_window, display_points,
                                   display_options, chart_layout, view_options, latency_types, 
                                   statistical_overlays, y_axis_scale, y_axis_range, sampling_strategy,
                                   mechanisms, message_sizes):
            from dash import ctx
            
            # Only generate charts if Run Analysis button clicked and we're on time series tab
            if not (n_clicks > 0 and active_tab == "timeseries-tab"):
                return html.Div([
                    html.Div([
                        html.H3("Time Series Charts Ready", style={'color': '#f0f6fc', 'margin-bottom': '20px'}),
                        html.P("Configure your analysis settings above and click 'Run Analysis' to generate interactive time series charts.", 
                               style={'color': '#6b7280', 'font-size': '1.1rem', 'line-height': '1.6', 'margin-bottom': '20px'}),
                        html.Div([
                            html.H4("Chart Features Available:", style={'color': '#f0f6fc', 'margin-bottom': '15px'}),
                            html.Ul([
                                html.Li("Interactive zoom and pan across all charts"),
                                html.Li("Multiple statistical overlays and anomaly detection"),
                                html.Li("Flexible layout options and sampling strategies"),
                                html.Li("Performance zones and percentile indicators"),
                                html.Li("Real-time statistics and distribution analysis")
                            ], style={'color': '#f0f6fc', 'padding-left': '20px', 'line-height': '1.8'})
                        ])
                    ], style={
                        'text-align': 'center', 
                        'padding': '40px 30px',
                        'background': 'linear-gradient(135deg, #f0f9ff 0%, #dbeafe 100%)',
                        'border-radius': '12px',
                        'margin': '20px 0',
                        'border': '2px dashed #3b82f6'
                    })
                ])
            
            # Handle None values for controls with defaults
            display_points = display_points if display_points is not None else 10000
            chart_layout = chart_layout if chart_layout is not None else 'separate'
            view_options = view_options if view_options is not None else ['sync_zoom']
            latency_types = latency_types if latency_types is not None else ['one_way']
            statistical_overlays = statistical_overlays if statistical_overlays is not None else ['percentile_bands']
            y_axis_scale = y_axis_scale if y_axis_scale is not None else 'log'
            y_axis_range = y_axis_range if y_axis_range is not None else 'auto'
            sampling_strategy = sampling_strategy if sampling_strategy is not None else 'uniform'
            
            return self.render_timeseries_charts(
                mechanisms, message_sizes, moving_avg_window, display_points, display_options, 
                chart_layout, view_options, latency_types, statistical_overlays, 
                y_axis_scale, y_axis_range, sampling_strategy
            )
        


        # Statistics panel update callback - only responds to Run Analysis
        @self.app.callback(
            Output('ts-stats-panel', 'children'),
            [Input('run-analysis-button', 'n_clicks'),
             Input('main-tabs', 'value')],
            [State('ts-view-options', 'value'),
             State('ts-latency-types', 'value'),
             State("mechanism-filter", "value"),
             State("message-size-filter", "value")],
            prevent_initial_call=True
        )
        @safe_computation(default_return=html.Div())
        def update_stats_panel(n_clicks, active_tab, view_options, latency_types, mechanisms, message_sizes):
            # Only show stats panel if Run Analysis was clicked and we're on time series tab
            if not (n_clicks > 0 and active_tab == "timeseries-tab"):
                return html.Div()
                
            if not view_options or 'show_stats' not in view_options:
                return html.Div()
            
            if not latency_types:
                latency_types = ['one_way']  # Default to one-way if nothing selected
            
            # Generate statistics overview
            streaming_df = self.filter_data(self.data_store['streaming_data'], mechanisms, message_sizes)
            
            if streaming_df.empty:
                return html.Div()
            
            # Create separate tables for each mechanism
            mechanism_tables = []
            for mechanism in sorted(streaming_df['mechanism'].unique()):
                mechanism_data = streaming_df[streaming_df['mechanism'] == mechanism]
                
                # Create stats for each latency type selected
                for latency_type in latency_types:
                    latency_col = 'one_way_latency_us' if latency_type == 'one_way' else 'round_trip_latency_us'
                    type_label = 'One-way' if latency_type == 'one_way' else 'Round-trip'
                    
                    # Skip if column doesn't exist or has no data
                    if latency_col not in mechanism_data.columns or mechanism_data[latency_col].isna().all():
                        continue
                    
                    stats_data = []
                    for msg_size in sorted(mechanism_data['message_size'].unique()):
                        subset_data = mechanism_data[mechanism_data['message_size'] == msg_size]
                        if len(subset_data) > 0 and not subset_data[latency_col].isna().all():
                            stats_data.append({
                                'Message Size (bytes)': f"{msg_size:,}",
                                'Count': f"{len(subset_data):,}",
                                'Mean (μs)': f"{subset_data[latency_col].mean():.2f}",
                                'Std (μs)': f"{subset_data[latency_col].std():.2f}",
                                'Min (μs)': f"{subset_data[latency_col].min():.2f}",
                                'Max (μs)': f"{subset_data[latency_col].max():.2f}",
                                'P95 (μs)': f"{subset_data[latency_col].quantile(0.95):.2f}",
                                'P99 (μs)': f"{subset_data[latency_col].quantile(0.99):.2f}"
                            })
                    
                    if stats_data:
                        # Color scheme for each mechanism
                        mechanism_colors = {
                            'PosixMessageQueue': 'rgba(59, 130, 246, 0.1)',
                            'SharedMemory': 'rgba(34, 197, 94, 0.1)', 
                            'TcpSocket': 'rgba(251, 146, 60, 0.1)',
                            'UnixDomainSocket': 'rgba(168, 85, 247, 0.1)'
                        }
                        
                        table = html.Div([
                            html.H5(f"{mechanism} - {type_label} Latency", 
                                   style={'margin-bottom': '10px', 'color': '#f0f6fc', 'font-weight': 'bold'}),
                            dash_table.DataTable(
                                data=stats_data,
                                columns=[{"name": col, "id": col} for col in stats_data[0].keys()],
                                style_cell={
                                    'textAlign': 'left', 
                                    'padding': '8px', 
                                    'font-size': '0.8rem',
                                    'backgroundColor': '#21262d',
                                    'color': '#f0f6fc',
                                    'border': '1px solid #30363d'
                                },
                                style_header={
                                    'backgroundColor': '#161b22', 
                                    'fontWeight': 'bold', 
                                    'color': '#f0f6fc',
                                    'border': '1px solid #30363d'
                                },
                                style_data={
                                    'backgroundColor': '#21262d', 
                                    'color': '#f0f6fc',
                                    'border': '1px solid #30363d'
                                },
                                style_table={'border-radius': '8px', 'overflow': 'hidden', 'backgroundColor': '#21262d'},
                                style_cell_conditional=[
                                    {'if': {'column_id': 'Message Size (bytes)'}, 'textAlign': 'right', 'font-weight': 'bold'},
                                    {'if': {'column_id': 'Count'}, 'textAlign': 'right'},
                                    {'if': {'column_id': 'Mean (μs)'}, 'textAlign': 'right'},
                                    {'if': {'column_id': 'Std (μs)'}, 'textAlign': 'right'},
                                    {'if': {'column_id': 'Min (μs)'}, 'textAlign': 'right'},
                                    {'if': {'column_id': 'Max (μs)'}, 'textAlign': 'right'},
                                    {'if': {'column_id': 'P95 (μs)'}, 'textAlign': 'right'},
                                    {'if': {'column_id': 'P99 (μs)'}, 'textAlign': 'right'}
                                ]
                            )
                        ], style={'margin-bottom': '20px'})
                        mechanism_tables.append(table)
            
            if not mechanism_tables:
                return html.Div()
            
            return html.Div([
                html.H4("Statistics Overview", style={'margin-bottom': '20px', 'color': '#1f2937', 'font-size': '1.1rem'}),
                html.Div(mechanism_tables)
            ], style={'margin-bottom': '30px', 'padding': '20px', 'background': '#f8fafc', 'border-radius': '8px', 'border': '1px solid #e2e8f0'})
        
        # Preset configuration buttons callbacks
        @self.app.callback(
            [Output('ts-chart-layout', 'value'),
             Output('ts-view-options', 'value'),
             Output('ts-statistical-overlays', 'value'),
             Output('ts-y-axis-scale', 'value'),
             Output('ts-sampling-strategy', 'value'),
             Output('ts-display-options', 'value'),
             Output('ts-latency-types', 'value')],
            [Input('preset-performance', 'n_clicks'),
             Input('preset-detailed', 'n_clicks'),
             Input('preset-statistical', 'n_clicks'),
             Input('preset-outliers', 'n_clicks'),
             Input('preset-reset', 'n_clicks')],
            [State('ts-view-options', 'value')],
            prevent_initial_call=True
        )
        def handle_preset_buttons(perf_clicks, detailed_clicks, stat_clicks, outlier_clicks, reset_clicks, current_view_options):
            import dash
            from dash import ctx
            
            if not ctx.triggered:
                raise PreventUpdate
            
            button_id = ctx.triggered[0]['prop_id'].split('.')[0]
            
            # Preserve the "show_stats" setting from current view options
            current_view_options = current_view_options or []
            preserve_stats = 'show_stats' in current_view_options
            
            def build_view_options(base_options):
                """Helper to build view options while preserving stats panel setting"""
                result = base_options.copy()
                # Force disable show_stats for all presets
                if 'show_stats' in result:
                    result.remove('show_stats')
                return result
            
            if button_id == 'preset-performance':
                # Performance Analysis preset
                return ('separate', build_view_options(['sync_zoom']), 
                       ['spike_detection'], 
                       'log', 'uniform', ['moving_avg'], ['round_trip'])
            
            elif button_id == 'preset-detailed':
                # Detailed Inspection preset
                return ('faceted', build_view_options(['sync_zoom']), 
                       [], 
                       'log', 'uniform', ['moving_avg'], ['round_trip'])
            
            elif button_id == 'preset-statistical':
                # Statistical Overview preset
                return ('faceted', build_view_options(['sync_zoom']), 
                       ['percentile_bands', 'spike_detection'], 
                       'log', 'peak_preserving', ['moving_avg'], ['one_way', 'round_trip'])
            
            elif button_id == 'preset-outliers':
                # Outlier Detection preset
                return ('separate', build_view_options(['sync_zoom']), 
                       ['percentile_bands', 'spike_detection', 'anomaly_detection'], 
                       'log', 'outlier_preserving', ['moving_avg'], ['round_trip'])
            
            elif button_id == 'preset-reset':
                # Reset to defaults (but preserve statistics panel setting)
                return ('separate', build_view_options(['sync_zoom']), 
                       ['percentile_bands', 'spike_detection'], 
                       'log', 'uniform', ['raw_dots', 'moving_avg'], ['one_way'])
            
            raise PreventUpdate
        
        # File browser modal callbacks
        @self.app.callback(
            Output('file-browser-modal', 'style'),
            [Input('browse-directory-button', 'n_clicks'),
             Input('file-browser-close', 'n_clicks'),
             Input('file-browser-cancel', 'n_clicks')],
            prevent_initial_call=False
        )
        def toggle_file_browser_modal(browse_clicks, close_clicks, cancel_clicks):
            ctx = dash.callback_context
            if not ctx.triggered:
                return {'display': 'none'}
            
            trigger_id = ctx.triggered[0]['prop_id'].split('.')[0]
            
            if trigger_id == 'browse-directory-button' and browse_clicks > 0:
                return {
                    'position': 'fixed',
                    'top': 0,
                    'left': 0,
                    'width': '100%',
                    'height': '100%',
                    'background-color': 'rgba(0, 0, 0, 0.5)',
                    'display': 'flex',
                    'justify-content': 'center',
                    'align-items': 'center',
                    'z-index': 1000
                }
            else:
                return {'display': 'none'}

        @self.app.callback(
            [Output('directory-listing', 'children'),
             Output('directory-breadcrumbs', 'children'),
             Output('current-directory-store', 'data')],
            [Input('current-directory-store', 'data'),
             Input('file-browser-modal', 'style')],
            prevent_initial_call=False
        )
        def update_directory_listing(current_dir, modal_style):
            if modal_style.get('display') == 'none':
                return [], [], current_dir
                
            items = list_directories(current_dir)
            breadcrumbs = get_directory_breadcrumbs(current_dir)
            
            # Create directory and file listing components
            listing_items = []
            for item_info in items:
                # Different styling for directories vs files
                if item_info['is_directory']:
                    # Clickable directories
                    item_style = {
                        'padding': '8px 12px',
                        'margin': '2px 0',
                        'cursor': 'pointer',
                        'border-radius': '4px',
                        'border': '1px solid transparent',
                        'background-color': '#ffffff',
                        'transition': 'background-color 0.2s'
                    }
                    item_id = {'type': 'directory-item', 'path': item_info['path']}
                    item_class = 'directory-item'
                else:
                    # Non-clickable files (informational)
                    item_style = {
                        'padding': '8px 12px',
                        'margin': '2px 0',
                        'border-radius': '4px',
                        'border': '1px solid #e5e7eb',
                        'background-color': '#f9fafb',
                        'color': '#6b7280',
                        'font-style': 'italic'
                    }
                    item_id = {'type': 'file-item', 'path': item_info['path']}
                    item_class = 'file-item'
                
                listing_items.append(
                    html.Div([
                        html.Span(item_info['icon'], style={'margin-right': '8px', 'font-size': '14px'}),
                        html.Span(item_info['name'], style={'font-size': '13px'})
                    ], 
                    id=item_id,
                    style=item_style,
                    className=item_class,
                    title=f"{'Navigate to' if item_info['is_directory'] else 'Data file:'} {item_info['path']}"
                    )
                )
            
            # Add a summary of files found
            file_count = sum(1 for item in items if item['is_file'])
            if file_count > 0:
                file_summary = html.Div([
                    html.Hr(style={'margin': '10px 0', 'border': 'none', 'border-top': '1px solid #e5e7eb'}),
                    html.Div([
                        html.Span(f"Found {file_count} data file{'s' if file_count != 1 else ''}", 
                                 style={'font-size': '12px', 'color': '#3fb950', 'font-weight': 'bold'})
                    ], style={'text-align': 'center', 'padding': '5px'})
                ], style={'margin-top': '10px'})
                listing_items.append(file_summary)
            
            # Create breadcrumb components  
            breadcrumb_items = []
            for i, crumb in enumerate(breadcrumbs):
                if i > 0:
                    breadcrumb_items.append(html.Span(' > ', style={'margin': '0 5px', 'color': '#9ca3af'}))
                
                breadcrumb_items.append(
                    html.Span(
                        crumb['name'],
                        id={'type': 'breadcrumb-item', 'path': crumb['path']},
                        style={
                            'cursor': 'pointer',
                            'color': '#3b82f6',
                            'text-decoration': 'underline',
                            'font-size': '14px'
                        }
                    )
                )
            
            return listing_items, breadcrumb_items, current_dir

        @self.app.callback(
            Output('current-directory-store', 'data', allow_duplicate=True),
            [Input({'type': 'directory-item', 'path': ALL}, 'n_clicks'),
             Input({'type': 'breadcrumb-item', 'path': ALL}, 'n_clicks')],
            [State('current-directory-store', 'data')],
            prevent_initial_call=True
        )
        def navigate_directory(dir_clicks, breadcrumb_clicks, current_dir):
            ctx = dash.callback_context
            if not ctx.triggered:
                raise PreventUpdate
                
            trigger_info = ctx.triggered[0]
            if trigger_info['value'] is None or trigger_info['value'] == 0:
                raise PreventUpdate
                
            # Parse the triggered component to get the path
            import json
            component_id = json.loads(trigger_info['prop_id'].split('.')[0])
            new_path = component_id['path']
            
            return new_path

        @self.app.callback(
            [Output('current-directory-display', 'children'),
             Output('selected-directory-store', 'data'),
             Output('file-browser-modal', 'style', allow_duplicate=True)],
            [Input('file-browser-select', 'n_clicks')],
            [State('current-directory-store', 'data')],
            prevent_initial_call=True
        )
        def select_directory(select_clicks, current_dir):
            if select_clicks == 0:
                raise PreventUpdate
                
            # Close the modal and update the selected directory
            return f"{current_dir}", current_dir, {'display': 'none'}
        
        # Data reload callback
        @self.app.callback(
            [Output('reload-status', 'children'),
             Output('mechanism-filter', 'options'),
             Output('mechanism-filter', 'value'),
             Output('message-size-filter', 'options'),
             Output('message-size-filter', 'value')],
            [Input('reload-data-button', 'n_clicks')],
            [State('selected-directory-store', 'data')],
            prevent_initial_call=True
        )
        def handle_data_reload(n_clicks, selected_directory):
            if n_clicks == 0:
                # No callback needed for initial state
                raise PreventUpdate
            
            # Use the selected directory from the file browser
            directory_path = selected_directory if selected_directory else '.'
            
            # Expand ~ for home directory
            if directory_path.startswith('~'):
                directory_path = os.path.expanduser(directory_path)
            
            # Attempt to reload data
            success, message = self.reload_data(directory_path)
            
            if success:
                # Update filter options with new data
                new_filter_options = self.get_filter_options()
                
                # Create status message with success styling
                status_div = html.Div([
                    html.P(message, style={
                        'color': '#059669', 
                        'font-size': '0.85rem', 
                        'margin': '8px 0',
                        'padding': '8px 12px',
                        'background-color': '#d1fae5',
                        'border': '1px solid #10b981',
                        'border-radius': '4px'
                    })
                ])
                
                # Return updated filter options and select all by default
                return (
                    status_div,
                    [{'label': m, 'value': m} for m in new_filter_options['mechanisms']],
                    new_filter_options['mechanisms'],  # Select all mechanisms by default
                    [{'label': str(s), 'value': s} for s in new_filter_options['message_sizes']],
                    new_filter_options['message_sizes']   # Select all message sizes by default
                )
            else:
                # Create status message with error styling
                status_div = html.Div([
                    html.P(message, style={
                        'color': '#dc2626', 
                        'font-size': '0.85rem', 
                        'margin': '8px 0',
                        'padding': '8px 12px',
                        'background-color': '#fef2f2',
                        'border': '1px solid #f87171',
                        'border-radius': '4px'
                    })
                ])
                
                # Keep existing filter options on error and maintain all selections
                current_filter_options = self.get_filter_options()
                return (
                    status_div,
                    [{'label': m, 'value': m} for m in current_filter_options['mechanisms']],
                    current_filter_options['mechanisms'],  # Keep all mechanisms selected
                    [{'label': str(s), 'value': s} for s in current_filter_options['message_sizes']],
                    current_filter_options['message_sizes']   # Keep all message sizes selected
                )

    def filter_data(self, df: pd.DataFrame, mechanisms: List, message_sizes: List) -> pd.DataFrame:
        """Apply filters to dataframe. Empty filter lists mean 'show all' for that dimension."""
        if df.empty:
            return df
        
        filtered = df.copy()
        
        # Apply filters only if selections are made. Empty list = show all data for that dimension
        if mechanisms is not None and len(mechanisms) > 0:
            filtered = filtered[filtered['mechanism'].isin(mechanisms)]
        # If mechanisms is None or empty, show all mechanisms
        
        if message_sizes is not None and len(message_sizes) > 0:
            filtered = filtered[filtered['message_size'].isin(message_sizes)]
        # If message_sizes is None or empty, show all message sizes
        
        return filtered
    
    @cached_computation()
    def generate_performance_insights(self, percentile_df: pd.DataFrame, throughput_df: pd.DataFrame = None, max_latency_df: pd.DataFrame = None) -> Dict:
        """Generate mechanism + message size specific performance recommendations based on lowest jitter and worst-case latency."""
        insights = {
            'best_mechanism': None,
            'best_latency': None,
            'best_max_latency': None,
            'best_max_mechanism': None,
            'best_consistency_mechanism': None,
            'best_throughput': None,
            'recommendations': [],
            'summary_stats': {}
        }
        
        if percentile_df.empty:
            insights['recommendations'].append("**NO DATA**: Select mechanisms and message sizes to generate performance insights")
            return insights
        
        # Get one-way latency data for analysis
        one_way_p50 = percentile_df[
            (percentile_df['latency_type'] == 'One-way') & 
            (percentile_df['percentile'] == 'P50 (Median)')
        ]
        one_way_p95 = percentile_df[
            (percentile_df['latency_type'] == 'One-way') & 
            (percentile_df['percentile'] == 'P95')
        ]
        
        if one_way_p50.empty or max_latency_df is None or max_latency_df.empty:
            insights['recommendations'].append("**INSUFFICIENT DATA**: Need P50, P95, and max latency data for analysis")
            return insights
        
        one_way_max = max_latency_df[max_latency_df['latency_type'] == 'One-way']
        if one_way_max.empty:
            insights['recommendations'].append("**INSUFFICIENT DATA**: Need max latency data for analysis")
            return insights
        
        # Calculate performance scores for each mechanism + message size combination
        performance_scores = []
        
        for message_size in sorted(one_way_p50['message_size'].unique()):
            size_p50 = one_way_p50[one_way_p50['message_size'] == message_size]
            size_p95 = one_way_p95[one_way_p95['message_size'] == message_size]
            size_max = one_way_max[one_way_max['message_size'] == message_size]
            
            for mechanism in size_p50['mechanism'].unique():
                p50_data = size_p50[size_p50['mechanism'] == mechanism]['latency_us']
                p95_data = size_p95[size_p95['mechanism'] == mechanism]['latency_us']
                max_data = size_max[size_max['mechanism'] == mechanism]['max_latency_us']
                
                if not p50_data.empty and not p95_data.empty and not max_data.empty:
                    p50_val = p50_data.iloc[0]
                    p95_val = p95_data.iloc[0]
                    max_val = max_data.iloc[0]
                    
                    # Calculate jitter (P95-P50 spread) - lower is better
                    jitter = p95_val - p50_val
                    
                    # Combined score: normalize both jitter and max latency, then combine
                    # Lower scores are better
                    performance_scores.append({
                        'mechanism': mechanism,
                        'message_size': message_size,
                        'p50': p50_val,
                        'p95': p95_val,
                        'max': max_val,
                        'jitter': jitter,
                        'score': max_val + jitter  # Simple combination: max latency + jitter penalty
                    })
        
        if not performance_scores:
            insights['recommendations'].append("**NO DATA**: Unable to calculate performance scores")
            return insights
        
        # Generate recommendations by message size
        message_sizes = sorted(set(score['message_size'] for score in performance_scores))
        
        for message_size in message_sizes:
            size_scores = [s for s in performance_scores if s['message_size'] == message_size]
            if len(size_scores) <= 1:
                continue
                
            # Sort by combined score (lower is better)
            size_scores.sort(key=lambda x: x['score'])
            
            best = size_scores[0]
            worst = size_scores[-1]
            
            # Create recommendation
            if len(size_scores) == 2:
                # Simple comparison between two mechanisms
                score_improvement = ((worst['score'] - best['score']) / worst['score']) * 100
                insights['recommendations'].append(
                    f"**{message_size}B**: {best['mechanism']} excels with {best['jitter']:.1f}μs jitter + {best['max']:.1f}μs max latency ({score_improvement:.0f}% better than {worst['mechanism']})"
                )
            else:
                # Multiple mechanisms - show best and mention others
                runner_up = size_scores[1]
                insights['recommendations'].append(
                    f"**{message_size}B**: {best['mechanism']} leads with {best['jitter']:.1f}μs jitter + {best['max']:.1f}μs max latency (beats {runner_up['mechanism']} and {len(size_scores)-2} others)"
                )
        
        # Overall analysis: find the mechanism that appears most often as winner
        mechanism_wins = {}
        for message_size in message_sizes:
            size_scores = [s for s in performance_scores if s['message_size'] == message_size]
            if size_scores:
                size_scores.sort(key=lambda x: x['score'])
                winner = size_scores[0]['mechanism']
                mechanism_wins[winner] = mechanism_wins.get(winner, 0) + 1
        
        if mechanism_wins:
            overall_winner = max(mechanism_wins, key=mechanism_wins.get)
            win_count = mechanism_wins[overall_winner]
            total_sizes = len(message_sizes)
            
            if win_count == total_sizes:
                insights['recommendations'].insert(0, f"**CHAMPION**: {overall_winner} dominates across ALL {total_sizes} message sizes with optimal jitter + max latency")
            elif win_count > total_sizes / 2:
                insights['recommendations'].insert(0, f"**TOP PERFORMER**: {overall_winner} wins {win_count}/{total_sizes} message sizes with superior jitter + max latency control")
            else:
                insights['recommendations'].insert(0, f"**CONTEXT MATTERS**: No single winner - choose {overall_winner} for {win_count} sizes, others context-dependent")
        
        # Add warning for extreme outliers
        extreme_outliers = [s for s in performance_scores if s['max'] > s['p50'] * 20 and s['max'] > 100]
        if extreme_outliers:
            worst_outlier = max(extreme_outliers, key=lambda x: x['max'] / x['p50'])
            ratio = worst_outlier['max'] / worst_outlier['p50']
            insights['recommendations'].insert(0, 
                f"**WARNING**: {worst_outlier['mechanism']} at {worst_outlier['message_size']}B has extreme spikes ({worst_outlier['max']:.0f}μs max vs {worst_outlier['p50']:.1f}μs typical = {ratio:.0f}x worse)"
            )
        
        # Throughput analysis for summary stats
        if throughput_df is not None and not throughput_df.empty:
            one_way_throughput = throughput_df[throughput_df['type'] == 'One-way']
            if not one_way_throughput.empty:
                best_throughput_row = one_way_throughput.loc[one_way_throughput['msgs_per_sec'].idxmax()]
                insights['best_throughput'] = {
                    'mechanism': best_throughput_row['mechanism'],
                    'msgs_per_sec': best_throughput_row['msgs_per_sec'],
                    'message_size': best_throughput_row['message_size']
                }
        
        # Store summary stats for cards
        if performance_scores:
            best_overall = min(performance_scores, key=lambda x: x['score'])
            insights['best_mechanism'] = best_overall['mechanism']
            insights['best_latency'] = best_overall['p50']
            insights['best_max_latency'] = best_overall['max']
            insights['best_max_mechanism'] = best_overall['mechanism']
            insights['summary_stats']['best_p50'] = best_overall['p50']
            insights['summary_stats']['best_max_latency'] = best_overall['max']
        
        return insights
    
    def create_performance_comparison_matrix(self, percentile_df: pd.DataFrame, max_latency_df: pd.DataFrame = None) -> tuple:
        """Create performance comparison matrices showing relative performance for P50 and max latency."""
        p50_matrix = pd.DataFrame()
        max_matrix = pd.DataFrame()
        
        # P50 Latency Matrix
        if not percentile_df.empty:
            # Focus on P50 one-way latency for the matrix
            p50_data = percentile_df[
                (percentile_df['latency_type'] == 'One-way') & 
                (percentile_df['percentile'] == 'P50 (Median)')
            ].copy()
            
            if not p50_data.empty:
                # Create pivot table with mechanisms as rows and message sizes as columns
                p50_matrix = p50_data.pivot(index='mechanism', columns='message_size', values='latency_us')
                
                # Calculate relative performance (percentage compared to best for each message size)
                for col in p50_matrix.columns:
                    col_data = p50_matrix[col].dropna()
                    if not col_data.empty:
                        best_value = col_data.min()
                        p50_matrix[col] = ((col_data - best_value) / best_value * 100).round(1)
                
                # Add overall score (average relative performance)
                p50_matrix['Overall Score'] = p50_matrix.mean(axis=1).round(1)
                p50_matrix = p50_matrix.fillna('-')
        
        # Max Latency Matrix
        if max_latency_df is not None and not max_latency_df.empty:
            # Focus on one-way max latency for the matrix
            max_data = max_latency_df[max_latency_df['latency_type'] == 'One-way'].copy()
            
            if not max_data.empty:
                # Create pivot table with mechanisms as rows and message sizes as columns
                max_matrix = max_data.pivot(index='mechanism', columns='message_size', values='max_latency_us')
                
                # Calculate relative performance (percentage compared to best for each message size)
                for col in max_matrix.columns:
                    col_data = max_matrix[col].dropna()
                    if not col_data.empty:
                        best_value = col_data.min()  # Lower max latency is better
                        max_matrix[col] = ((col_data - best_value) / best_value * 100).round(1)
                
                # Add overall score (average relative performance)
                max_matrix['Overall Score'] = max_matrix.mean(axis=1).round(1)
                max_matrix = max_matrix.fillna('-')
        
        return p50_matrix, max_matrix
    
    def create_summary_cards(self, insights: Dict, percentile_df: pd.DataFrame, max_latency_df: pd.DataFrame = None) -> html.Div:
        """Create summary performance cards for the top of the dashboard."""
        if not insights or percentile_df.empty:
            return html.Div()
        
        # Best Mechanism Card
        best_mechanism_card = html.Div([
            html.Div([
                html.H4("Best Overall", style={'margin': '0', 'color': '#059669', 'font-size': '0.9rem'}),
                html.H2(insights.get('best_mechanism', 'N/A'), style={'margin': '5px 0', 'color': '#065f46'}),
                html.P(f"P50: {insights.get('best_latency', 0):.1f}μs", style={'margin': '0', 'color': '#8b949e', 'font-size': '0.9rem'})
            ], style={
                'background': 'linear-gradient(135deg, #d1fae5 0%, #a7f3d0 100%)',
                'padding': '20px', 'border-radius': '12px', 'text-align': 'center',
                'box-shadow': '0 4px 6px rgba(0, 0, 0, 0.1)', 'border': '1px solid #10b981'
            }),
            html.P("The mechanism with the best balance of typical performance (P50) and consistency. Combines low latency with predictable behavior - ideal for most applications.", 
                   style={'margin': '8px 0 0 0', 'font-size': '0.75rem', 'color': '#8b949e', 'font-style': 'italic', 'line-height': '1.3', 'text-align': 'center'})
        ], style={'flex': '1', 'min-width': '0'})
        
        # Performance Range Card for Best Mechanism
        best_mechanism = insights.get('best_mechanism', 'N/A')
        
        # Get all latency data for the best mechanism (not just P50)
        if best_mechanism != 'N/A' and not percentile_df.empty:
            best_mechanism_data = percentile_df[
                (percentile_df['mechanism'] == best_mechanism) & 
                (percentile_df['latency_type'] == 'One-way')
            ]
            
            if not best_mechanism_data.empty:
                min_latency = best_mechanism_data['latency_us'].min()
                max_latency = best_mechanism_data['latency_us'].max()
                p99_latency = best_mechanism_data[
                    best_mechanism_data['percentile'] == 'P99'
                ]['latency_us'].iloc[0] if len(best_mechanism_data[
                    best_mechanism_data['percentile'] == 'P99'
                ]) > 0 else 0
            else:
                min_latency = max_latency = p99_latency = 0
        else:
            min_latency = max_latency = p99_latency = 0
        
        performance_card = html.Div([
            html.Div([
                html.H4("Performance Range", style={'margin': '0', 'color': '#1e40af', 'font-size': '0.9rem'}),
                html.H2(f"{min_latency:.1f} - {max_latency:.1f}μs", style={'margin': '5px 0', 'color': '#1e3a8a'}),
                html.P(f"{best_mechanism} (P99: {p99_latency:.1f}μs)", style={'margin': '0', 'color': '#8b949e', 'font-size': '0.9rem'})
            ], style={
                'background': 'linear-gradient(135deg, #dbeafe 0%, #bfdbfe 100%)',
                'padding': '20px', 'border-radius': '12px', 'text-align': 'center',
                'box-shadow': '0 4px 6px rgba(0, 0, 0, 0.1)', 'border': '1px solid #3b82f6'
            }),
            html.P("Shows the latency spread for the best mechanism. Smaller ranges indicate more predictable performance. P99 represents near-worst-case latency.", 
                   style={'margin': '8px 0 0 0', 'font-size': '0.75rem', 'color': '#8b949e', 'font-style': 'italic', 'line-height': '1.3', 'text-align': 'center'})
        ], style={'flex': '1', 'min-width': '0'})
        
        # Throughput Card
        throughput_info = insights.get('best_throughput', {})
        throughput_card = html.Div([
            html.Div([
                html.H4("Peak Throughput", style={'margin': '0', 'color': '#7c2d12', 'font-size': '0.9rem'}),
                html.H2(f"{throughput_info.get('msgs_per_sec', 0):,.0f}", style={'margin': '5px 0', 'color': '#92400e'}),
                html.P(f"msgs/sec ({throughput_info.get('mechanism', 'N/A')})", style={'margin': '0', 'color': '#8b949e', 'font-size': '0.9rem'})
            ], style={
                'background': 'linear-gradient(135deg, #fef3c7 0%, #fde68a 100%)',
                'padding': '20px', 'border-radius': '12px', 'text-align': 'center',
                'box-shadow': '0 4px 6px rgba(0, 0, 0, 0.1)', 'border': '1px solid #f59e0b'
            }),
            html.P("Highest message rate achieved across all mechanisms. Important for high-volume applications that prioritize throughput over latency.", 
                   style={'margin': '8px 0 0 0', 'font-size': '0.75rem', 'color': '#8b949e', 'font-style': 'italic', 'line-height': '1.3', 'text-align': 'center'})
        ], style={'flex': '1', 'min-width': '0'})
        
        # Max Latency Card
        max_latency_card = html.Div()
        if max_latency_df is not None and not max_latency_df.empty:
            one_way_max = max_latency_df[max_latency_df['latency_type'] == 'One-way']
            if not one_way_max.empty:
                best_max_latency = one_way_max['max_latency_us'].min()
                best_max_mechanism = one_way_max.loc[one_way_max['max_latency_us'].idxmin(), 'mechanism']
                
                max_latency_card = html.Div([
                    html.Div([
                        html.H4("Best Max Latency", style={'margin': '0', 'color': '#dc2626', 'font-size': '0.9rem'}),
                        html.H2(f"{best_max_latency:.1f}μs", style={'margin': '5px 0', 'color': '#991b1b'}),
                        html.P(f"{best_max_mechanism}", style={'margin': '0', 'color': '#8b949e', 'font-size': '0.9rem'})
                    ], style={
                        'background': 'linear-gradient(135deg, #fee2e2 0%, #fecaca 100%)',
                        'padding': '20px', 'border-radius': '12px', 'text-align': 'center',
                        'box-shadow': '0 4px 6px rgba(0, 0, 0, 0.1)', 'border': '1px solid #dc2626'
                    }),
                    html.P("Mechanism with the lowest worst-case latency. Critical for real-time systems where maximum response time must be guaranteed.", 
                           style={'margin': '8px 0 0 0', 'font-size': '0.75rem', 'color': '#8b949e', 'font-style': 'italic', 'line-height': '1.3', 'text-align': 'center'})
                ], style={'flex': '1', 'min-width': '0'})
        
        # Insights Card (adjusted width)
        cards = [best_mechanism_card, performance_card, throughput_card]
        if max_latency_card.children:  # Only add if max latency data exists
            cards.append(max_latency_card)
        
        return html.Div(cards, style={
            'display': 'flex',
            'flex-direction': 'row', 
            'justify-content': 'space-between',
            'align-items': 'stretch',
            'gap': '15px',
            'margin-bottom': '30px',
            'flex-wrap': 'nowrap'  # Prevent wrapping to ensure single row
        })

    @cached_computation()
    def calculate_percentiles_from_streaming(self, mechanisms: List, message_sizes: List):
        """Calculate percentiles dynamically from streaming data."""
        logger.info(f"calculate_percentiles_from_streaming: Original data shape: {self.data_store['streaming_data'].shape}")
        streaming_df = self.filter_data(self.data_store['streaming_data'], mechanisms, message_sizes)
        logger.info(f"calculate_percentiles_from_streaming: Filtered data shape: {streaming_df.shape}")
        
        if streaming_df.empty:
            logger.warning("Filtered streaming data is empty")
            return pd.DataFrame()
        
        # Calculate percentiles for both one-way and round-trip latencies
        percentile_data = []
        
        # Group by mechanism and message size
        for (mechanism, message_size), group in streaming_df.groupby(['mechanism', 'message_size']):
            # Calculate one-way latency percentiles
            if 'one_way_latency_us' in group.columns:
                one_way_latencies = group['one_way_latency_us'].dropna()
                if len(one_way_latencies) > 0:
                    percentile_data.extend([
                        {
                            'mechanism': mechanism,
                            'message_size': message_size,
                            'latency_type': 'One-way',
                            'percentile': 'P50 (Median)',
                            'latency_us': np.percentile(one_way_latencies, 50),
                            'sample_count': len(one_way_latencies)
                        },
                        {
                            'mechanism': mechanism,
                            'message_size': message_size,
                            'latency_type': 'One-way',
                            'percentile': 'P95',
                            'latency_us': np.percentile(one_way_latencies, 95),
                            'sample_count': len(one_way_latencies)
                        },
                        {
                            'mechanism': mechanism,
                            'message_size': message_size,
                            'latency_type': 'One-way',
                            'percentile': 'P99',
                            'latency_us': np.percentile(one_way_latencies, 99),
                            'sample_count': len(one_way_latencies)
                        }
                    ])
            
            # Calculate round-trip latency percentiles
            if 'round_trip_latency_us' in group.columns:
                round_trip_latencies = group['round_trip_latency_us'].dropna()
                if len(round_trip_latencies) > 0:
                    percentile_data.extend([
                        {
                            'mechanism': mechanism,
                            'message_size': message_size,
                            'latency_type': 'Round-trip',
                            'percentile': 'P50 (Median)',
                            'latency_us': np.percentile(round_trip_latencies, 50),
                            'sample_count': len(round_trip_latencies)
                        },
                        {
                            'mechanism': mechanism,
                            'message_size': message_size,
                            'latency_type': 'Round-trip',
                            'percentile': 'P95',
                            'latency_us': np.percentile(round_trip_latencies, 95),
                            'sample_count': len(round_trip_latencies)
                        },
                        {
                            'mechanism': mechanism,
                            'message_size': message_size,
                            'latency_type': 'Round-trip',
                            'percentile': 'P99',
                            'latency_us': np.percentile(round_trip_latencies, 99),
                            'sample_count': len(round_trip_latencies)
                        }
                    ])
        
        return pd.DataFrame(percentile_data)

    @cached_computation()
    def calculate_max_latencies(self, mechanisms: List, message_sizes: List):
        """Calculate maximum latencies from streaming data for performance comparison."""
        logger.info(f"calculate_max_latencies: Original data shape: {self.data_store['streaming_data'].shape}")
        streaming_df = self.filter_data(self.data_store['streaming_data'], mechanisms, message_sizes)
        logger.info(f"calculate_max_latencies: Filtered data shape: {streaming_df.shape}")
        
        if streaming_df.empty:
            logger.warning("Filtered streaming data is empty for max latency calculation")
            return pd.DataFrame()
        
        max_latency_data = []
        
        # Group by mechanism and message size
        for (mechanism, message_size), group in streaming_df.groupby(['mechanism', 'message_size']):
            # Calculate one-way maximum latency
            if 'one_way_latency_us' in group.columns:
                one_way_latencies = group['one_way_latency_us'].dropna()
                if len(one_way_latencies) > 0:
                    max_latency_data.append({
                        'mechanism': mechanism,
                        'message_size': message_size,
                        'latency_type': 'One-way',
                        'max_latency_us': one_way_latencies.max(),
                        'sample_count': len(one_way_latencies)
                    })
            
            # Calculate round-trip maximum latency
            if 'round_trip_latency_us' in group.columns:
                round_trip_latencies = group['round_trip_latency_us'].dropna()
                if len(round_trip_latencies) > 0:
                    max_latency_data.append({
                        'mechanism': mechanism,
                        'message_size': message_size,
                        'latency_type': 'Round-trip',
                        'max_latency_us': round_trip_latencies.max(),
                        'sample_count': len(round_trip_latencies)
                    })
        
        return pd.DataFrame(max_latency_data)

    def create_pivot_table(self, data: List[Dict], value_col: str, index_col: str = 'message_size', 
                          columns_col: str = 'mechanism', round_digits: int = 2) -> pd.DataFrame:
        """Create a pivot table with message size as rows and mechanisms as columns."""
        if not data:
            return pd.DataFrame()
        
        df = pd.DataFrame(data)
        if df.empty:
            return pd.DataFrame()
        
        # Create pivot table
        pivot = df.pivot_table(
            values=value_col, 
            index=index_col, 
            columns=columns_col, 
            aggfunc='mean',  # In case of duplicates, take the mean
            fill_value=None
        )
        
        # Round values
        if round_digits > 0:
            pivot = pivot.round(round_digits)
        
        # Reset index to make message_size a column
        pivot = pivot.reset_index()
        
        # Convert column names to strings and clean up
        pivot.columns = [str(col) if col != index_col else 'Message Size (bytes)' for col in pivot.columns]
        pivot.columns.name = None  # Remove the column name
        
        return pivot

    def render_summary_tab(self, mechanisms: List, message_sizes: List):
        """Orchestrator method - coordinates all summary tab rendering."""
        logger.info(f"render_summary_tab called with: mechanisms={mechanisms}, message_sizes={message_sizes}")
        
        # 1. Data preparation
        filtered_data = self._prepare_summary_data(mechanisms, message_sizes)
        if not filtered_data:
            return self._create_no_data_message()
        
        # 2. Statistical calculations  
        stats = self._calculate_summary_statistics(filtered_data)
        
        # 3. Generate insights and cards
        insights_section = self._create_insights_section(stats)
        
        # 4. Create comparison matrices
        comparison_section = self._create_comparison_matrices(stats)
        
        # 5. Generate charts with tables integrated
        charts_section = self._create_summary_charts(filtered_data, stats)
        
        # 6. Assemble final layout
        return self._assemble_summary_layout([
            insights_section, comparison_section,
            charts_section
        ])
    
    def _prepare_summary_data(self, mechanisms: List, message_sizes: List) -> Dict:
        """Prepare and validate data for summary analysis."""
        # Calculate percentiles from streaming data (much more robust!)
        percentile_df = self.calculate_percentiles_from_streaming(mechanisms, message_sizes)
        logger.info(f"Percentile df shape: {percentile_df.shape}, empty: {percentile_df.empty}")
        
        if percentile_df.empty:
            logger.warning("Percentile df is empty, returning no data message")
            return None
        
        # Sort mechanisms by median one-way latency (best performing first)
        mechanism_order = (percentile_df[
            (percentile_df['latency_type'] == 'One-way') & 
            (percentile_df['percentile'] == 'P50 (Median)')
        ].groupby('mechanism')['latency_us'].mean().sort_values().index.tolist())
        
        # Filter data by user selections
        summary_df = self.filter_data(self.data_store['summary_data'], mechanisms, message_sizes)
        streaming_df = self.filter_data(self.data_store['streaming_data'], mechanisms, message_sizes)
        
        # Sort message sizes from smallest to largest for consistent ordering
        message_size_order = sorted(streaming_df['message_size'].unique()) if not streaming_df.empty else []
        
        return {
            'percentile_df': percentile_df,
            'summary_df': summary_df,
            'streaming_df': streaming_df,
            'mechanism_order': mechanism_order,
            'message_size_order': message_size_order
        }
    
    def _create_no_data_message(self) -> html.Div:
        """Create a no data available message."""
        return html.Div([
            html.H3("No Data Available"),
            html.P("No streaming data available for the selected filters. Try adjusting your filter selections.", 
                   style={'color': '#6b7280', 'fontSize': '1.1rem', 'textAlign': 'center', 'padding': '40px'})
        ], style={'textAlign': 'center', 'padding': '50px'})
    
    def _calculate_summary_statistics(self, filtered_data: Dict) -> Dict:
        """Calculate all statistical metrics needed for summary tab."""
        percentile_df = filtered_data['percentile_df']
        summary_df = filtered_data['summary_df']
        streaming_df = filtered_data['streaming_df']
        mechanisms = streaming_df['mechanism'].unique() if not streaming_df.empty else []
        message_sizes = streaming_df['message_size'].unique() if not streaming_df.empty else []
        
        # Calculate throughput data
        throughput_df = self._calculate_throughput_data(summary_df, streaming_df)
        
        # Generate structured latency data for pivot tables
        latency_data = self._generate_latency_data_for_tables(streaming_df, summary_df)
        
        # Generate structured throughput data for pivot tables  
        throughput_data = self._generate_throughput_data_for_tables(throughput_df)
        
        # Calculate max latency data for comparison
        max_latency_df = self.calculate_max_latencies(list(mechanisms), list(message_sizes))
        
        return {
            'percentile_df': percentile_df,
            'throughput_df': throughput_df,
            'max_latency_df': max_latency_df,
            'streaming_df': streaming_df,
            'summary_df': summary_df,
            'latency_data': latency_data,
            'throughput_data': throughput_data
        }
    
    def _calculate_throughput_data(self, summary_df: pd.DataFrame, streaming_df: pd.DataFrame) -> pd.DataFrame:
        """Calculate throughput data from summary or streaming data."""
        throughput_data = []
        
        # Use summary data if available, otherwise calculate from streaming data
        if not summary_df.empty:
            for _, row in summary_df.iterrows():
                if not pd.isna(row['one_way_msgs_per_sec']):
                    throughput_data.append({
                        'mechanism': row['mechanism'],
                        'message_size': row['message_size'],
                        'type': 'One-way',
                        'msgs_per_sec': row['one_way_msgs_per_sec'],
                        'bytes_per_sec': row['one_way_bytes_per_sec'],
                        'source': 'Summary Data'
                    })
                if not pd.isna(row['round_trip_msgs_per_sec']):
                    throughput_data.append({
                        'mechanism': row['mechanism'],
                        'message_size': row['message_size'],
                        'type': 'Round-trip',
                        'msgs_per_sec': row['round_trip_msgs_per_sec'],
                        'bytes_per_sec': row['round_trip_bytes_per_sec'],
                        'source': 'Summary Data'
                    })
        
        # If no summary throughput data, estimate from streaming data
        if not throughput_data and not streaming_df.empty:
            logger.info("No summary throughput data found, estimating from streaming data...")
            
            # Group by mechanism and message_size to estimate throughput
            for (mechanism, message_size), group in streaming_df.groupby(['mechanism', 'message_size']):
                if len(group) > 10:  # Need sufficient samples
                    # Calculate time window for throughput estimation
                    if 'timestamp_ns' in group.columns:
                        time_span_sec = (group['timestamp_ns'].max() - group['timestamp_ns'].min()) / 1e9
                        if time_span_sec > 0:
                            msgs_per_sec = len(group) / time_span_sec
                            bytes_per_sec = msgs_per_sec * message_size
                            
                            throughput_data.append({
                                'mechanism': mechanism,
                                'message_size': message_size,
                                'type': 'Estimated',
                                'msgs_per_sec': msgs_per_sec,
                                'bytes_per_sec': bytes_per_sec,
                                'source': 'Streaming Data Estimate'
                            })
        
        throughput_df = pd.DataFrame(throughput_data) if throughput_data else pd.DataFrame()
        
        # Filter out invalid throughput data
        if not throughput_df.empty:
            throughput_df = throughput_df.dropna(subset=['msgs_per_sec', 'bytes_per_sec'])
            throughput_df = throughput_df[(throughput_df['msgs_per_sec'] > 0) & (throughput_df['bytes_per_sec'] > 0)]
        
        return throughput_df
    
    def _generate_latency_data_for_tables(self, streaming_df: pd.DataFrame, summary_df: pd.DataFrame) -> List:
        """Generate structured latency data for pivot tables."""
        latency_data = []
        
        if not streaming_df.empty:
            # Calculate average latencies from streaming data
            avg_latencies = streaming_df.groupby(['mechanism', 'message_size']).agg({
                'one_way_latency_us': 'mean',
                'round_trip_latency_us': 'mean'
            }).reset_index()
            
            for _, row in avg_latencies.iterrows():
                if not pd.isna(row['one_way_latency_us']):
                    latency_data.append({
                        'mechanism': row['mechanism'],
                        'message_size': int(row['message_size']),
                        'type': 'Average',
                        'source': 'One-way',
                        'latency_us': row['one_way_latency_us']
                    })
                if not pd.isna(row['round_trip_latency_us']):
                    latency_data.append({
                        'mechanism': row['mechanism'],
                        'message_size': int(row['message_size']),
                        'type': 'Average',
                        'source': 'Round-trip',
                        'latency_us': row['round_trip_latency_us']
                    })
            
            # Calculate maximum latencies from streaming data
            max_latencies = streaming_df.groupby(['mechanism', 'message_size']).agg({
                'one_way_latency_us': 'max',
                'round_trip_latency_us': 'max'
            }).reset_index()
            
            for _, row in max_latencies.iterrows():
                if not pd.isna(row['one_way_latency_us']):
                    latency_data.append({
                        'mechanism': row['mechanism'],
                        'message_size': int(row['message_size']),
                        'type': 'Maximum',
                        'source': 'One-way',
                        'latency_us': row['one_way_latency_us']
                    })
                if not pd.isna(row['round_trip_latency_us']):
                    latency_data.append({
                        'mechanism': row['mechanism'],
                        'message_size': int(row['message_size']),
                        'type': 'Maximum',
                        'source': 'Round-trip',
                        'latency_us': row['round_trip_latency_us']
                    })
        
        return latency_data
    
    def _generate_throughput_data_for_tables(self, throughput_df: pd.DataFrame) -> List:
        """Generate structured throughput data for pivot tables."""
        throughput_data = []
        
        if not throughput_df.empty:
            for _, row in throughput_df.iterrows():
                # The throughput DataFrame already has the correct structure
                throughput_data.append({
                    'mechanism': row['mechanism'],
                    'message_size': int(row['message_size']),
                    'type': row['type'],  # Already contains 'One-way' or 'Round-trip'
                    'msgs_per_sec': row['msgs_per_sec'],
                    'bytes_per_sec': row['bytes_per_sec']
                })
        
        return throughput_data
    
    def _create_latency_tables(self, latency_data: List) -> html.Div:
        """Create latency performance tables."""
        if not latency_data:
            return html.Div()
            
        latency_df = pd.DataFrame(latency_data)
        
        # Filter data for each table type
        max_oneway_data = latency_df[(latency_df['type'] == 'Maximum') & (latency_df['source'] == 'One-way')]
        max_roundtrip_data = latency_df[(latency_df['type'] == 'Maximum') & (latency_df['source'] == 'Round-trip')]
        avg_oneway_data = latency_df[(latency_df['type'] == 'Average') & (latency_df['source'] == 'One-way')]
        avg_roundtrip_data = latency_df[(latency_df['type'] == 'Average') & (latency_df['source'] == 'Round-trip')]
        
        # Create pivot tables
        max_oneway_pivot = self.create_pivot_table(max_oneway_data.to_dict('records'), 'latency_us')
        max_roundtrip_pivot = self.create_pivot_table(max_roundtrip_data.to_dict('records'), 'latency_us')
        avg_oneway_pivot = self.create_pivot_table(avg_oneway_data.to_dict('records'), 'latency_us')
        avg_roundtrip_pivot = self.create_pivot_table(avg_roundtrip_data.to_dict('records'), 'latency_us')
        
        # Common table style - Grafana dark theme
        table_style = {
            'style_table': {'overflowX': 'auto', 'width': '100%', 'backgroundColor': '#21262d'},
            'style_cell': {
                'textAlign': 'center', 
                'padding': '8px', 
                'fontFamily': 'Inter, sans-serif', 
                'fontSize': '12px',
                'backgroundColor': '#21262d',
                'color': '#f0f6fc',
                'border': '1px solid #30363d'
            },
            'style_header': {
                'backgroundColor': '#161b22', 
                'fontWeight': 'bold', 
                'color': '#f0f6fc',
                'border': '1px solid #30363d'
            },
            'style_data_conditional': [{'if': {'row_index': 'odd'}, 'backgroundColor': '#161b22'}],
            'sort_action': "native"
        }
        
        # Create individual table components
        latency_tables = []
        
        # Max Latency Tables (side by side)
        if not max_oneway_pivot.empty or not max_roundtrip_pivot.empty:
            max_tables_row = html.Div([
                html.H4("Maximum Latency (μs)", style={'color': '#f85149', 'margin-bottom': '15px', 'text-align': 'center'}),
                html.Div([
                    # One-way max latency table
                    html.Div([
                        html.H5("One-way", style={'color': '#f0f6fc', 'margin-bottom': '10px', 'text-align': 'center'}),
                        dash_table.DataTable(
                            id='max-oneway-latency-table',
                            data=max_oneway_pivot.to_dict('records') if not max_oneway_pivot.empty else [],
                            columns=[{"name": i, "id": i} for i in max_oneway_pivot.columns] if not max_oneway_pivot.empty else [],
                            **table_style
                        ) if not max_oneway_pivot.empty else html.Div("No data available", style={'text-align': 'center', 'color': '#8b949e'})
                    ], style={'width': '48%', 'display': 'inline-block', 'margin-right': '4%'}),
                    
                    # Round-trip max latency table
                    html.Div([
                        html.H5("Round-trip", style={'color': '#f0f6fc', 'margin-bottom': '10px', 'text-align': 'center'}),
                        dash_table.DataTable(
                            id='max-roundtrip-latency-table',
                            data=max_roundtrip_pivot.to_dict('records') if not max_roundtrip_pivot.empty else [],
                            columns=[{"name": i, "id": i} for i in max_roundtrip_pivot.columns] if not max_roundtrip_pivot.empty else [],
                            **table_style
                        ) if not max_roundtrip_pivot.empty else html.Div("No data available", style={'text-align': 'center', 'color': '#8b949e'})
                    ], style={'width': '48%', 'display': 'inline-block'})
                ], style={'margin-bottom': '30px'})
            ])
            latency_tables.append(max_tables_row)
        
        # Average Latency Tables (side by side)
        if not avg_oneway_pivot.empty or not avg_roundtrip_pivot.empty:
            avg_tables_row = html.Div([
                html.H4("Average Latency (μs)", style={'color': '#3fb950', 'margin-bottom': '15px', 'text-align': 'center'}),
                html.Div([
                    # One-way average latency table
                    html.Div([
                        html.H5("One-way", style={'color': '#f0f6fc', 'margin-bottom': '10px', 'text-align': 'center'}),
                        dash_table.DataTable(
                            id='avg-oneway-latency-table',
                            data=avg_oneway_pivot.to_dict('records') if not avg_oneway_pivot.empty else [],
                            columns=[{"name": i, "id": i} for i in avg_oneway_pivot.columns] if not avg_oneway_pivot.empty else [],
                            **table_style
                        ) if not avg_oneway_pivot.empty else html.Div("No data available", style={'text-align': 'center', 'color': '#8b949e'})
                    ], style={'width': '48%', 'display': 'inline-block', 'margin-right': '4%'}),
                    
                    # Round-trip average latency table
                    html.Div([
                        html.H5("Round-trip", style={'color': '#f0f6fc', 'margin-bottom': '10px', 'text-align': 'center'}),
                        dash_table.DataTable(
                            id='avg-roundtrip-latency-table',
                            data=avg_roundtrip_pivot.to_dict('records') if not avg_roundtrip_pivot.empty else [],
                            columns=[{"name": i, "id": i} for i in avg_roundtrip_pivot.columns] if not avg_roundtrip_pivot.empty else [],
                            **table_style
                        ) if not avg_roundtrip_pivot.empty else html.Div("No data available", style={'text-align': 'center', 'color': '#8b949e'})
                    ], style={'width': '48%', 'display': 'inline-block'})
                ])
            ])
            latency_tables.append(avg_tables_row)
        
        return html.Div([
            html.H3("Latency Performance Data"),
            html.P("Latency measurements organized by message size and mechanism. Lower values indicate better performance.",
                   style={'color': '#8b949e', 'margin-bottom': '20px', 'fontSize': '0.95rem'}),
            *latency_tables
        ], className="dash-table-container")
    
    def _create_throughput_tables(self, throughput_data: List) -> html.Div:
        """Create throughput performance tables."""
        if not throughput_data:
            return html.Div()
            
        throughput_df_data = pd.DataFrame(throughput_data)
        
        # Filter data for each table type
        oneway_throughput_data = throughput_df_data[throughput_df_data['type'] == 'One-way']
        roundtrip_throughput_data = throughput_df_data[throughput_df_data['type'] == 'Round-trip']
        
        # Create pivot tables for messages/sec
        oneway_msgs_pivot = self.create_pivot_table(oneway_throughput_data.to_dict('records'), 'msgs_per_sec', round_digits=1)
        roundtrip_msgs_pivot = self.create_pivot_table(roundtrip_throughput_data.to_dict('records'), 'msgs_per_sec', round_digits=1)
        
        # Create pivot tables for bytes/sec (convert to MB/s for readability)
        oneway_throughput_mb = oneway_throughput_data.copy()
        roundtrip_throughput_mb = roundtrip_throughput_data.copy()
        if not oneway_throughput_mb.empty:
            oneway_throughput_mb['bytes_per_sec'] = oneway_throughput_mb['bytes_per_sec'] / 1_000_000  # Convert to MB/s
        if not roundtrip_throughput_mb.empty:
            roundtrip_throughput_mb['bytes_per_sec'] = roundtrip_throughput_mb['bytes_per_sec'] / 1_000_000  # Convert to MB/s
            
        oneway_bytes_pivot = self.create_pivot_table(oneway_throughput_mb.to_dict('records'), 'bytes_per_sec', round_digits=1)
        roundtrip_bytes_pivot = self.create_pivot_table(roundtrip_throughput_mb.to_dict('records'), 'bytes_per_sec', round_digits=1)
        
        # Common table style for throughput tables - Grafana dark theme
        throughput_table_style = {
            'style_table': {'overflowX': 'auto', 'width': '100%', 'backgroundColor': '#21262d'},
            'style_cell': {
                'textAlign': 'center', 
                'padding': '8px', 
                'fontFamily': 'Inter, sans-serif', 
                'fontSize': '12px',
                'backgroundColor': '#21262d',
                'color': '#f0f6fc',
                'border': '1px solid #30363d'
            },
            'style_header': {
                'backgroundColor': '#161b22', 
                'fontWeight': 'bold', 
                'color': '#f0f6fc',
                'border': '1px solid #30363d'
            },
            'style_data_conditional': [{'if': {'row_index': 'odd'}, 'backgroundColor': '#161b22'}],
            'sort_action': "native"
        }
        
        throughput_tables = []
        
        # Messages/sec Tables (side by side)
        if not oneway_msgs_pivot.empty or not roundtrip_msgs_pivot.empty:
            msgs_tables_row = html.Div([
                html.H4("Message Throughput (messages/sec)", style={'color': '#58a6ff', 'margin-bottom': '15px', 'text-align': 'center'}),
                html.Div([
                    # One-way messages/sec table
                    html.Div([
                        html.H5("One-way", style={'color': '#f0f6fc', 'margin-bottom': '10px', 'text-align': 'center'}),
                        dash_table.DataTable(
                            id='oneway-msgs-throughput-table',
                            data=oneway_msgs_pivot.to_dict('records') if not oneway_msgs_pivot.empty else [],
                            columns=[{"name": i, "id": i} for i in oneway_msgs_pivot.columns] if not oneway_msgs_pivot.empty else [],
                            **throughput_table_style
                        ) if not oneway_msgs_pivot.empty else html.Div("No data available", style={'text-align': 'center', 'color': '#8b949e'})
                    ], style={'width': '48%', 'display': 'inline-block', 'margin-right': '4%'}),
                    
                    # Round-trip messages/sec table
                    html.Div([
                        html.H5("Round-trip", style={'color': '#f0f6fc', 'margin-bottom': '10px', 'text-align': 'center'}),
                        dash_table.DataTable(
                            id='roundtrip-msgs-throughput-table',
                            data=roundtrip_msgs_pivot.to_dict('records') if not roundtrip_msgs_pivot.empty else [],
                            columns=[{"name": i, "id": i} for i in roundtrip_msgs_pivot.columns] if not roundtrip_msgs_pivot.empty else [],
                            **throughput_table_style
                        ) if not roundtrip_msgs_pivot.empty else html.Div("No data available", style={'text-align': 'center', 'color': '#8b949e'})
                    ], style={'width': '48%', 'display': 'inline-block'})
                ], style={'margin-bottom': '30px'})
            ])
            throughput_tables.append(msgs_tables_row)
        
        # Bytes/sec Tables (side by side)
        if not oneway_bytes_pivot.empty or not roundtrip_bytes_pivot.empty:
            bytes_tables_row = html.Div([
                html.H4("Data Throughput (MB/sec)", style={'color': '#58a6ff', 'margin-bottom': '15px', 'text-align': 'center'}),
                html.Div([
                    # One-way bytes/sec table
                    html.Div([
                        html.H5("One-way", style={'color': '#f0f6fc', 'margin-bottom': '10px', 'text-align': 'center'}),
                        dash_table.DataTable(
                            id='oneway-bytes-throughput-table',
                            data=oneway_bytes_pivot.to_dict('records') if not oneway_bytes_pivot.empty else [],
                            columns=[{"name": i, "id": i} for i in oneway_bytes_pivot.columns] if not oneway_bytes_pivot.empty else [],
                            **throughput_table_style
                        ) if not oneway_bytes_pivot.empty else html.Div("No data available", style={'text-align': 'center', 'color': '#8b949e'})
                    ], style={'width': '48%', 'display': 'inline-block', 'margin-right': '4%'}),
                    
                    # Round-trip bytes/sec table
                    html.Div([
                        html.H5("Round-trip", style={'color': '#f0f6fc', 'margin-bottom': '10px', 'text-align': 'center'}),
                        dash_table.DataTable(
                            id='roundtrip-bytes-throughput-table',
                            data=roundtrip_bytes_pivot.to_dict('records') if not roundtrip_bytes_pivot.empty else [],
                            columns=[{"name": i, "id": i} for i in roundtrip_bytes_pivot.columns] if not roundtrip_bytes_pivot.empty else [],
                            **throughput_table_style
                        ) if not roundtrip_bytes_pivot.empty else html.Div("No data available", style={'text-align': 'center', 'color': '#8b949e'})
                    ], style={'width': '48%', 'display': 'inline-block'})
                ])
            ])
            throughput_tables.append(bytes_tables_row)
        
        return html.Div([
            html.H3("Throughput Performance Data"),
            html.P(f"Throughput measurements organized by message size and mechanism. Higher values indicate better performance.",
                   style={'color': '#8b949e', 'margin-bottom': '20px', 'fontSize': '0.95rem'}),
            *throughput_tables
        ], className="dash-table-container")
    
    def _create_insights_section(self, stats: Dict) -> html.Div:
        """Generate insights and summary cards section."""
        percentile_df = stats['percentile_df']
        throughput_df = stats['throughput_df'] 
        max_latency_df = stats['max_latency_df']
        
        # Generate insights and summary cards
        insights = self.generate_performance_insights(percentile_df, throughput_df, max_latency_df)
        summary_cards = self.create_summary_cards(insights, percentile_df, max_latency_df)
        
        return html.Div([
            html.H2("Performance Overview", style={'color': '#f0f6fc', 'margin-bottom': '20px', 'font-size': '1.5rem'}),
            summary_cards,
            html.Div([
                html.H2("Performance Insights", style={'color': '#f0f6fc', 'margin-bottom': '20px', 'font-size': '1.5rem'}),
                html.Div([
                    html.Div([
                        html.H4("Recommendations:", style={'color': '#3fb950', 'margin-bottom': '15px'}),
                        html.Ul([
                            html.Li(rec, style={'margin-bottom': '8px', 'color': '#f0f6fc'}) 
                            for rec in insights.get('recommendations', ['Select data to see recommendations'])
                        ], style={'padding-left': '20px'})
                    ], style={
                        'background': 'linear-gradient(135deg, #21262d 0%, #161b22 100%)',
                        'padding': '25px', 'border-radius': '12px',
                        'box-shadow': '0 4px 6px rgba(0, 0, 0, 0.3)', 'border': '1px solid #3fb950'
                    })
                ], className="chart-container")
            ], style={'margin-bottom': '40px'})
        ], style={'margin-bottom': '40px'})
    
    def _create_comparison_matrices(self, stats: Dict) -> html.Div:
        """Create performance comparison matrices section."""
        percentile_df = stats['percentile_df']
        max_latency_df = stats['max_latency_df']
        
        p50_comparison_matrix, max_comparison_matrix = self.create_performance_comparison_matrix(percentile_df, max_latency_df)
        
        # Create performance comparison matrix tables
        comparison_matrix_div = self._create_comparison_matrix_tables(p50_comparison_matrix, max_comparison_matrix)
        
        return html.Div([
            html.H2("Head-to-Head Comparison", style={'color': '#f0f6fc', 'margin-bottom': '20px', 'font-size': '1.5rem'}),
            comparison_matrix_div
        ], style={'margin-bottom': '40px'})
    
    def _create_summary_charts(self, filtered_data: Dict, stats: Dict) -> html.Div:
        """Generate latency and throughput charts with integrated tables.""" 
        streaming_df = filtered_data['streaming_df']
        mechanism_order = filtered_data['mechanism_order']
        message_size_order = filtered_data['message_size_order']
        throughput_df = stats['throughput_df']
        latency_data = stats.get('latency_data', [])
        throughput_data = stats.get('throughput_data', [])
        
        # Create latency charts
        avg_latency_fig, max_latency_fig = self._create_latency_charts(streaming_df, mechanism_order, message_size_order)
        
        # Create throughput charts  
        msgs_fig, bytes_fig = self._create_throughput_charts(throughput_df, mechanism_order, message_size_order)
        
        # Create latency tables
        latency_tables_div = self._create_latency_tables(latency_data)
        
        # Create throughput tables
        throughput_tables_div = self._create_throughput_tables(throughput_data)
        
        return html.Div([
            # LATENCY ANALYSIS SECTION
            html.Div([
                html.H2("Latency Analysis", style={'color': '#f0f6fc', 'margin-bottom': '20px', 'font-size': '1.5rem'}),
                html.Div([
                    dcc.Graph(id='avg-latency-chart', figure=self.apply_dark_theme(avg_latency_fig))
                ], className="chart-container"),
                html.Div([
                    dcc.Graph(id='max-latency-chart', figure=self.apply_dark_theme(max_latency_fig))
                ], className="chart-container"),
                latency_tables_div
            ], style={'margin-bottom': '40px'}),
            
            # THROUGHPUT ANALYSIS SECTION
            html.Div([
                html.H2("Throughput Analysis", style={'color': '#f0f6fc', 'margin-bottom': '20px', 'font-size': '1.5rem'}),
                html.Div([
                    dcc.Graph(id='throughput-msgs-chart', figure=self.apply_dark_theme(msgs_fig))
                ], className="chart-container"),
                html.Div([
                    dcc.Graph(id='throughput-bytes-chart', figure=self.apply_dark_theme(bytes_fig))
                ], className="chart-container"),
                throughput_tables_div
            ], style={'margin-bottom': '40px'})
        ])
    
    def _create_latency_charts(self, streaming_df: pd.DataFrame, mechanism_order: List, message_size_order: List):
        """Create average and maximum latency charts."""
        latency_data = []
        
        if not streaming_df.empty:
            # Calculate average and max latencies from streaming data
            for (mechanism, message_size), group in streaming_df.groupby(['mechanism', 'message_size']):
                if 'one_way_latency_us' in group.columns:
                    one_way_data = group['one_way_latency_us'].dropna()
                    if len(one_way_data) > 0:
                        latency_data.append({
                            'mechanism': mechanism,
                            'message_size': message_size,
                            'type': 'Average',
                            'latency_us': one_way_data.mean(),
                            'source': 'One-way'
                        })
                        latency_data.append({
                            'mechanism': mechanism,
                            'message_size': message_size,
                            'type': 'Maximum',
                            'latency_us': one_way_data.max(),
                            'source': 'One-way'
                        })
                
                if 'round_trip_latency_us' in group.columns:
                    round_trip_data = group['round_trip_latency_us'].dropna()
                    if len(round_trip_data) > 0:
                        latency_data.append({
                            'mechanism': mechanism,
                            'message_size': message_size,
                            'type': 'Average',
                            'latency_us': round_trip_data.mean(),
                            'source': 'Round-trip'
                        })
                        latency_data.append({
                            'mechanism': mechanism,
                            'message_size': message_size,
                            'type': 'Maximum',
                            'latency_us': round_trip_data.max(),
                            'source': 'Round-trip'
                        })
        
        # Create latency DataFrames and charts
        if latency_data:
            latency_df = pd.DataFrame(latency_data)
            
            # Average latency chart
            avg_latency_fig = px.bar(
                latency_df[latency_df['type'] == 'Average'],
                x='mechanism',
                y='latency_us',
                color='source',
                facet_col='message_size',
                title="Average Latency (μs) by Mechanism and Message Size (Ordered by Performance)",
                labels={'latency_us': 'Average Latency (μs)', 'message_size': 'Message Size (bytes)'},
                category_orders={'mechanism': mechanism_order, 'message_size': message_size_order},
                color_discrete_map={'One-way': '#10b981', 'Round-trip': '#8b5cf6'}
            )
            # Update facet labels to show only the message size value
            avg_latency_fig.for_each_annotation(lambda a: a.update(text=a.text.split("=")[-1]))
            # Add consistent gray gridlines to all axes
            avg_latency_fig.update_layout(
                xaxis=dict(showgrid=True, gridcolor='gray', gridwidth=1),
                yaxis=dict(showgrid=True, gridcolor='gray', gridwidth=1)
            )
            avg_latency_fig.for_each_xaxis(lambda x: x.update(showgrid=True, gridcolor='gray', gridwidth=1))
            avg_latency_fig.for_each_yaxis(lambda y: y.update(showgrid=True, gridcolor='gray', gridwidth=1))
            
            # Maximum latency chart
            max_latency_fig = px.bar(
                latency_df[latency_df['type'] == 'Maximum'],
                x='mechanism',
                y='latency_us',
                color='source',
                facet_col='message_size',
                title="Maximum Latency (μs) by Mechanism and Message Size (Ordered by Performance)",
                labels={'latency_us': 'Maximum Latency (μs)', 'message_size': 'Message Size (bytes)'},
                category_orders={'mechanism': mechanism_order, 'message_size': message_size_order},
                color_discrete_map={'One-way': '#10b981', 'Round-trip': '#8b5cf6'}
            )
            # Update facet labels to show only the message size value
            max_latency_fig.for_each_annotation(lambda a: a.update(text=a.text.split("=")[-1]))
            # Add consistent gray gridlines to all axes
            max_latency_fig.update_layout(
                xaxis=dict(showgrid=True, gridcolor='gray', gridwidth=1),
                yaxis=dict(showgrid=True, gridcolor='gray', gridwidth=1)
            )
            max_latency_fig.for_each_xaxis(lambda x: x.update(showgrid=True, gridcolor='gray', gridwidth=1))
            max_latency_fig.for_each_yaxis(lambda y: y.update(showgrid=True, gridcolor='gray', gridwidth=1))
        else:
            avg_latency_fig = go.Figure().add_annotation(
                text="No latency data available",
                x=0.5, y=0.5, xref="paper", yref="paper", showarrow=False,
                font=dict(size=16, color="#6b7280")
            )
            max_latency_fig = go.Figure().add_annotation(
                text="No latency data available",
                x=0.5, y=0.5, xref="paper", yref="paper", showarrow=False,
                font=dict(size=16, color="#6b7280")
            )
        
        return avg_latency_fig, max_latency_fig
    
    def _create_throughput_charts(self, throughput_df: pd.DataFrame, mechanism_order: List, message_size_order: List):
        """Create throughput charts."""
        # Generate throughput charts
        if not throughput_df.empty:
            msgs_fig = px.bar(
                throughput_df,
                x='mechanism',
                y='msgs_per_sec',
                color='type',
                facet_col='message_size',
                title="Throughput (Messages/sec) by Mechanism and Message Size (Ordered by Performance)",
                labels={'msgs_per_sec': 'Messages per Second', 'message_size': 'Message Size (bytes)'},
                category_orders={'mechanism': mechanism_order, 'message_size': message_size_order},
                color_discrete_map={'One-way': '#10b981', 'Round-trip': '#8b5cf6'}
            )
            # Update facet labels to show only the message size value
            msgs_fig.for_each_annotation(lambda a: a.update(text=a.text.split("=")[-1]))
            # Add consistent gray gridlines to all axes
            msgs_fig.update_layout(
                xaxis=dict(showgrid=True, gridcolor='gray', gridwidth=1),
                yaxis=dict(showgrid=True, gridcolor='gray', gridwidth=1)
            )
            msgs_fig.for_each_xaxis(lambda x: x.update(showgrid=True, gridcolor='gray', gridwidth=1))
            msgs_fig.for_each_yaxis(lambda y: y.update(showgrid=True, gridcolor='gray', gridwidth=1))
                
            bytes_fig = px.bar(
                throughput_df,
                x='mechanism',
                y='bytes_per_sec',
                color='type',
                facet_col='message_size',
                title="Throughput (Bytes/sec) by Mechanism and Message Size (Ordered by Performance)",
                labels={'bytes_per_sec': 'Bytes per Second', 'message_size': 'Message Size (bytes)'},
                category_orders={'mechanism': mechanism_order, 'message_size': message_size_order},
                color_discrete_map={'One-way': '#10b981', 'Round-trip': '#8b5cf6'}
            )
            # Update facet labels to show only the message size value
            bytes_fig.for_each_annotation(lambda a: a.update(text=a.text.split("=")[-1]))
            # Add consistent gray gridlines to all axes
            bytes_fig.update_layout(
                xaxis=dict(showgrid=True, gridcolor='gray', gridwidth=1),
                yaxis=dict(showgrid=True, gridcolor='gray', gridwidth=1)
            )
            bytes_fig.for_each_xaxis(lambda x: x.update(showgrid=True, gridcolor='gray', gridwidth=1))
            bytes_fig.for_each_yaxis(lambda y: y.update(showgrid=True, gridcolor='gray', gridwidth=1))
        else:
            msgs_fig = go.Figure().add_annotation(
                text="No throughput data available",
                x=0.5, y=0.5, xref="paper", yref="paper", showarrow=False,
                font=dict(size=16, color="#6b7280")
            )
            bytes_fig = go.Figure().add_annotation(
                text="No throughput data available",
                x=0.5, y=0.5, xref="paper", yref="paper", showarrow=False,
                font=dict(size=16, color="#6b7280")
            )
        
        return msgs_fig, bytes_fig
    
    def _create_summary_tables(self, stats: Dict) -> html.Div:
        """Create summary pivot tables section."""
        percentile_df = stats['percentile_df'] 
        streaming_df = stats['streaming_df']
        throughput_df = stats['throughput_df']
        
        latency_data = stats.get('latency_data', [])
        throughput_data = stats.get('throughput_data', [])
        
        tables_divs = []

        # 1. Latency Tables (your original tables)
        if latency_data:
            latency_df = pd.DataFrame(latency_data)
            
            # Filter data for each table type
            max_oneway_data = latency_df[(latency_df['type'] == 'Maximum') & (latency_df['source'] == 'One-way')]
            max_roundtrip_data = latency_df[(latency_df['type'] == 'Maximum') & (latency_df['source'] == 'Round-trip')]
            avg_oneway_data = latency_df[(latency_df['type'] == 'Average') & (latency_df['source'] == 'One-way')]
            avg_roundtrip_data = latency_df[(latency_df['type'] == 'Average') & (latency_df['source'] == 'Round-trip')]
            
            # Create pivot tables
            max_oneway_pivot = self.create_pivot_table(max_oneway_data.to_dict('records'), 'latency_us')
            max_roundtrip_pivot = self.create_pivot_table(max_roundtrip_data.to_dict('records'), 'latency_us')
            avg_oneway_pivot = self.create_pivot_table(avg_oneway_data.to_dict('records'), 'latency_us')
            avg_roundtrip_pivot = self.create_pivot_table(avg_roundtrip_data.to_dict('records'), 'latency_us')
            
            # Common table style - Grafana dark theme
            table_style = {
                'style_table': {'overflowX': 'auto', 'width': '100%', 'backgroundColor': '#21262d'},
                'style_cell': {
                    'textAlign': 'center', 
                    'padding': '8px', 
                    'fontFamily': 'Inter, sans-serif', 
                    'fontSize': '12px',
                    'backgroundColor': '#21262d',
                    'color': '#f0f6fc',
                    'border': '1px solid #30363d'
                },
                'style_header': {
                    'backgroundColor': '#161b22', 
                    'fontWeight': 'bold', 
                    'color': '#f0f6fc',
                    'border': '1px solid #30363d'
                },
                'style_data_conditional': [{'if': {'row_index': 'odd'}, 'backgroundColor': '#161b22'}],
                'sort_action': "native"
            }
            
            # Create individual table components
            latency_tables = []
            
            # Max Latency Tables (side by side)
            if not max_oneway_pivot.empty or not max_roundtrip_pivot.empty:
                max_tables_row = html.Div([
                    html.H4("Maximum Latency (μs)", style={'color': '#f85149', 'margin-bottom': '15px', 'text-align': 'center'}),
                    html.Div([
                        # One-way max latency table
                        html.Div([
                            html.H5("One-way", style={'color': '#f0f6fc', 'margin-bottom': '10px', 'text-align': 'center'}),
                            dash_table.DataTable(
                                id='max-oneway-latency-table',
                                data=max_oneway_pivot.to_dict('records') if not max_oneway_pivot.empty else [],
                                columns=[{"name": i, "id": i} for i in max_oneway_pivot.columns] if not max_oneway_pivot.empty else [],
                                **table_style
                            ) if not max_oneway_pivot.empty else html.Div("No data available", style={'text-align': 'center', 'color': '#8b949e'})
                        ], style={'width': '48%', 'display': 'inline-block', 'margin-right': '4%'}),
                        
                        # Round-trip max latency table
                        html.Div([
                            html.H5("Round-trip", style={'color': '#f0f6fc', 'margin-bottom': '10px', 'text-align': 'center'}),
                            dash_table.DataTable(
                                id='max-roundtrip-latency-table',
                                data=max_roundtrip_pivot.to_dict('records') if not max_roundtrip_pivot.empty else [],
                                columns=[{"name": i, "id": i} for i in max_roundtrip_pivot.columns] if not max_roundtrip_pivot.empty else [],
                                **table_style
                            ) if not max_roundtrip_pivot.empty else html.Div("No data available", style={'text-align': 'center', 'color': '#8b949e'})
                        ], style={'width': '48%', 'display': 'inline-block'})
                    ], style={'margin-bottom': '30px'})
                ])
                latency_tables.append(max_tables_row)
            
            # Average Latency Tables (side by side)
            if not avg_oneway_pivot.empty or not avg_roundtrip_pivot.empty:
                avg_tables_row = html.Div([
                    html.H4("Average Latency (μs)", style={'color': '#3fb950', 'margin-bottom': '15px', 'text-align': 'center'}),
                    html.Div([
                        # One-way average latency table
                        html.Div([
                            html.H5("One-way", style={'color': '#f0f6fc', 'margin-bottom': '10px', 'text-align': 'center'}),
                            dash_table.DataTable(
                                id='avg-oneway-latency-table',
                                data=avg_oneway_pivot.to_dict('records') if not avg_oneway_pivot.empty else [],
                                columns=[{"name": i, "id": i} for i in avg_oneway_pivot.columns] if not avg_oneway_pivot.empty else [],
                                **table_style
                            ) if not avg_oneway_pivot.empty else html.Div("No data available", style={'text-align': 'center', 'color': '#8b949e'})
                        ], style={'width': '48%', 'display': 'inline-block', 'margin-right': '4%'}),
                        
                        # Round-trip average latency table
                        html.Div([
                            html.H5("Round-trip", style={'color': '#f0f6fc', 'margin-bottom': '10px', 'text-align': 'center'}),
                            dash_table.DataTable(
                                id='avg-roundtrip-latency-table',
                                data=avg_roundtrip_pivot.to_dict('records') if not avg_roundtrip_pivot.empty else [],
                                columns=[{"name": i, "id": i} for i in avg_roundtrip_pivot.columns] if not avg_roundtrip_pivot.empty else [],
                                **table_style
                            ) if not avg_roundtrip_pivot.empty else html.Div("No data available", style={'text-align': 'center', 'color': '#8b949e'})
                        ], style={'width': '48%', 'display': 'inline-block'})
                    ])
                ])
                latency_tables.append(avg_tables_row)
            
            latency_table_div = html.Div([
                html.H3("Latency Performance Data"),
                html.P("Latency measurements organized by message size and mechanism. Lower values indicate better performance.",
                       style={'color': '#8b949e', 'margin-bottom': '20px', 'fontSize': '0.95rem'}),
                *latency_tables
            ], className="dash-table-container")
            tables_divs.append(latency_table_div)
        
        # 2. Throughput Tables (your original tables)
        if throughput_data:
            throughput_df_data = pd.DataFrame(throughput_data)
            
            # Filter data for each table type
            oneway_throughput_data = throughput_df_data[throughput_df_data['type'] == 'One-way']
            roundtrip_throughput_data = throughput_df_data[throughput_df_data['type'] == 'Round-trip']
            
            # Create pivot tables for messages/sec
            oneway_msgs_pivot = self.create_pivot_table(oneway_throughput_data.to_dict('records'), 'msgs_per_sec', round_digits=1)
            roundtrip_msgs_pivot = self.create_pivot_table(roundtrip_throughput_data.to_dict('records'), 'msgs_per_sec', round_digits=1)
            
            # Create pivot tables for bytes/sec (convert to MB/s for readability)
            oneway_throughput_mb = oneway_throughput_data.copy()
            roundtrip_throughput_mb = roundtrip_throughput_data.copy()
            if not oneway_throughput_mb.empty:
                oneway_throughput_mb['bytes_per_sec'] = oneway_throughput_mb['bytes_per_sec'] / 1_000_000  # Convert to MB/s
            if not roundtrip_throughput_mb.empty:
                roundtrip_throughput_mb['bytes_per_sec'] = roundtrip_throughput_mb['bytes_per_sec'] / 1_000_000  # Convert to MB/s
                
            oneway_bytes_pivot = self.create_pivot_table(oneway_throughput_mb.to_dict('records'), 'bytes_per_sec', round_digits=1)
            roundtrip_bytes_pivot = self.create_pivot_table(roundtrip_throughput_mb.to_dict('records'), 'bytes_per_sec', round_digits=1)
            
            # Common table style for throughput tables - Grafana dark theme
            throughput_table_style = {
                'style_table': {'overflowX': 'auto', 'width': '100%', 'backgroundColor': '#21262d'},
                'style_cell': {
                    'textAlign': 'center', 
                    'padding': '8px', 
                    'fontFamily': 'Inter, sans-serif', 
                    'fontSize': '12px',
                    'backgroundColor': '#21262d',
                    'color': '#f0f6fc',
                    'border': '1px solid #30363d'
                },
                'style_header': {
                    'backgroundColor': '#161b22', 
                    'fontWeight': 'bold', 
                    'color': '#f0f6fc',
                    'border': '1px solid #30363d'
                },
                'style_data_conditional': [{'if': {'row_index': 'odd'}, 'backgroundColor': '#161b22'}],
                'sort_action': "native"
            }
            
            throughput_tables = []
            
            # Messages/sec Tables (side by side)
            if not oneway_msgs_pivot.empty or not roundtrip_msgs_pivot.empty:
                msgs_tables_row = html.Div([
                    html.H4("Message Throughput (messages/sec)", style={'color': '#58a6ff', 'margin-bottom': '15px', 'text-align': 'center'}),
                    html.Div([
                        # One-way messages/sec table
                        html.Div([
                            html.H5("One-way", style={'color': '#f0f6fc', 'margin-bottom': '10px', 'text-align': 'center'}),
                            dash_table.DataTable(
                                id='oneway-msgs-throughput-table',
                                data=oneway_msgs_pivot.to_dict('records') if not oneway_msgs_pivot.empty else [],
                                columns=[{"name": i, "id": i} for i in oneway_msgs_pivot.columns] if not oneway_msgs_pivot.empty else [],
                                **throughput_table_style
                            ) if not oneway_msgs_pivot.empty else html.Div("No data available", style={'text-align': 'center', 'color': '#8b949e'})
                        ], style={'width': '48%', 'display': 'inline-block', 'margin-right': '4%'}),
                        
                        # Round-trip messages/sec table
                        html.Div([
                            html.H5("Round-trip", style={'color': '#f0f6fc', 'margin-bottom': '10px', 'text-align': 'center'}),
                            dash_table.DataTable(
                                id='roundtrip-msgs-throughput-table',
                                data=roundtrip_msgs_pivot.to_dict('records') if not roundtrip_msgs_pivot.empty else [],
                                columns=[{"name": i, "id": i} for i in roundtrip_msgs_pivot.columns] if not roundtrip_msgs_pivot.empty else [],
                                **throughput_table_style
                            ) if not roundtrip_msgs_pivot.empty else html.Div("No data available", style={'text-align': 'center', 'color': '#8b949e'})
                        ], style={'width': '48%', 'display': 'inline-block'})
                    ], style={'margin-bottom': '30px'})
                ])
                throughput_tables.append(msgs_tables_row)
            
            # Bytes/sec Tables (side by side)
            if not oneway_bytes_pivot.empty or not roundtrip_bytes_pivot.empty:
                bytes_tables_row = html.Div([
                    html.H4("Data Throughput (MB/sec)", style={'color': '#58a6ff', 'margin-bottom': '15px', 'text-align': 'center'}),
                    html.Div([
                        # One-way bytes/sec table
                        html.Div([
                            html.H5("One-way", style={'color': '#f0f6fc', 'margin-bottom': '10px', 'text-align': 'center'}),
                            dash_table.DataTable(
                                id='oneway-bytes-throughput-table',
                                data=oneway_bytes_pivot.to_dict('records') if not oneway_bytes_pivot.empty else [],
                                columns=[{"name": i, "id": i} for i in oneway_bytes_pivot.columns] if not oneway_bytes_pivot.empty else [],
                                **throughput_table_style
                            ) if not oneway_bytes_pivot.empty else html.Div("No data available", style={'text-align': 'center', 'color': '#8b949e'})
                        ], style={'width': '48%', 'display': 'inline-block', 'margin-right': '4%'}),
                        
                        # Round-trip bytes/sec table
                        html.Div([
                            html.H5("Round-trip", style={'color': '#f0f6fc', 'margin-bottom': '10px', 'text-align': 'center'}),
                            dash_table.DataTable(
                                id='roundtrip-bytes-throughput-table',
                                data=roundtrip_bytes_pivot.to_dict('records') if not roundtrip_bytes_pivot.empty else [],
                                columns=[{"name": i, "id": i} for i in roundtrip_bytes_pivot.columns] if not roundtrip_bytes_pivot.empty else [],
                                **throughput_table_style
                            ) if not roundtrip_bytes_pivot.empty else html.Div("No data available", style={'text-align': 'center', 'color': '#8b949e'})
                        ], style={'width': '48%', 'display': 'inline-block'})
                    ])
                ])
                throughput_tables.append(bytes_tables_row)
            
            throughput_table_div = html.Div([
                html.H3("Throughput Performance Data"),
                html.P(f"Throughput measurements organized by message size and mechanism. Higher values indicate better performance.",
                       style={'color': '#8b949e', 'margin-bottom': '20px', 'fontSize': '0.95rem'}),
                *throughput_tables
            ], className="dash-table-container")
            tables_divs.append(throughput_table_div)

        return html.Div(tables_divs, style={'margin-bottom': '40px'})
    
    def _create_comparison_matrix_tables(self, p50_matrix, max_matrix) -> html.Div:
        """Create comparison matrix tables."""
        matrix_tables = []
        
        # P50 Latency Comparison Matrix
        if not p50_matrix.empty:
            # Reset index to make mechanism names a column
            p50_display = p50_matrix.reset_index()
            p50_display.columns.name = None  # Remove column name
            
            # Format the matrix for better display
            p50_display = p50_display.round(1)
            for col in p50_display.columns:
                if col != 'mechanism' and col != 'Overall Score':
                    p50_display[col] = p50_display[col].apply(lambda x: f"+{x}%" if x != '-' and x > 0 else f"{x}%" if x != '-' else x)
            
            p50_table = dash_table.DataTable(
                id='p50-comparison-table',
                data=p50_display.to_dict('records'),
                columns=[{"name": str(i), "id": str(i)} for i in p50_display.columns],
                style_table={'overflowX': 'auto', 'backgroundColor': '#21262d'},
                style_cell={
                    'textAlign': 'center', 
                    'padding': '12px', 
                    'fontFamily': 'Inter, sans-serif', 
                    'fontSize': '14px',
                    'backgroundColor': '#21262d',
                    'color': '#f0f6fc',
                    'border': '1px solid #30363d'
                },
                style_header={
                    'backgroundColor': '#161b22',
                    'fontWeight': 'bold',
                    'color': '#58a6ff',
                    'border': '1px solid #30363d'
                },
                style_data_conditional=[
                    {'if': {'row_index': 'odd'}, 'backgroundColor': '#161b22'},
                    {'if': {'column_id': 'mechanism'}, 'textAlign': 'left', 'fontWeight': 'bold'},
                    {'if': {'column_id': 'Overall Score'}, 'backgroundColor': '#2d2013', 'fontWeight': 'bold', 'color': '#ffd700'},
                ],
                sort_action="native",
            )
            
            matrix_tables.append(html.Div([
                html.H4("P50 Latency Comparison", style={'color': '#58a6ff', 'margin-bottom': '10px'}),
                html.P("Relative P50 latency performance. Lower percentages = better typical performance.",
                       style={'color': '#8b949e', 'margin-bottom': '15px', 'fontSize': '0.9rem'}),
                p50_table
            ], style={'margin-bottom': '30px'}))
        
        # Max Latency Comparison Matrix
        if not max_matrix.empty:
            # Reset index to make mechanism names a column
            max_display = max_matrix.reset_index()
            max_display.columns.name = None  # Remove column name
            
            # Format the matrix for better display
            max_display = max_display.round(1)
            for col in max_display.columns:
                if col != 'mechanism' and col != 'Overall Score':
                    max_display[col] = max_display[col].apply(lambda x: f"+{x}%" if x != '-' and x > 0 else f"{x}%" if x != '-' else x)
            
            max_table = dash_table.DataTable(
                id='max-comparison-table',
                data=max_display.to_dict('records'),
                columns=[{"name": str(i), "id": str(i)} for i in max_display.columns],
                style_table={'overflowX': 'auto', 'backgroundColor': '#21262d'},
                style_cell={
                    'textAlign': 'center',
                    'padding': '12px',
                    'fontFamily': 'Inter, sans-serif',
                    'fontSize': '14px',
                    'backgroundColor': '#21262d',
                    'color': '#f0f6fc',
                    'border': '1px solid #30363d'
                },
                style_header={
                    'backgroundColor': '#161b22',
                    'fontWeight': 'bold',
                    'color': '#f85149',
                    'border': '1px solid #30363d'
                },
                style_data_conditional=[
                    {'if': {'row_index': 'odd'}, 'backgroundColor': '#161b22'},
                    {'if': {'column_id': 'mechanism'}, 'textAlign': 'left', 'fontWeight': 'bold'},
                    {'if': {'column_id': 'Overall Score'}, 'backgroundColor': '#2d1b1b', 'fontWeight': 'bold', 'color': '#f85149'},
                ],
                sort_action="native",
            )
            
            matrix_tables.append(html.Div([
                html.H4("Max Latency Comparison", style={'color': '#f85149', 'margin-bottom': '10px'}),
                html.P("Relative maximum latency performance. Lower percentages = better worst-case performance.",
                       style={'color': '#8b949e', 'margin-bottom': '15px', 'fontSize': '0.9rem'}),
                max_table
            ], style={'margin-bottom': '30px'}))
            
        return html.Div(matrix_tables, className="dash-table-container")
    
    def _assemble_summary_layout(self, sections: List) -> html.Div:
        """Assemble final summary tab layout."""
        return html.Div([
            # Loading wrappers for each section
            dcc.Loading(
                id="performance-overview-loading",
                type="circle", 
                children=sections[0] if len(sections) > 0 else html.Div(),
                style={'minHeight': '150px'}
            ),
            dcc.Loading(
                id="comparison-matrix-loading",
                type="cube",
                children=sections[1] if len(sections) > 1 else html.Div(),
                style={'minHeight': '200px'}
            ),
            dcc.Loading(
                id="latency-analysis-loading",
                type="default",
                children=sections[2] if len(sections) > 2 else html.Div(),
                style={'minHeight': '300px'}
            ),
            dcc.Loading(
                id="throughput-analysis-loading",
                type="default", 
                children=sections[3] if len(sections) > 3 else html.Div(),
                style={'minHeight': '300px'}
            )
        ])
    
    # ===== EXTRACTED PURE FUNCTIONS FROM TIME SERIES TAB =====
    
    @staticmethod
    def _apply_sampling_strategy(df: pd.DataFrame, strategy: str, max_points: int) -> pd.DataFrame:
        """Apply different sampling strategies to the dataframe."""
        if len(df) <= max_points:
            return df
            
        if strategy == 'uniform':
            # Standard uniform sampling
            step_size = len(df) // max_points
            return df.iloc[::step_size].copy()
        
        elif strategy == 'peak_preserving':
            # Preserve peaks and valleys in the data
            uniform_sample = df.iloc[::len(df)//max_points].copy()
            # Add peaks (highest values)
            peaks = df.nlargest(max_points//4, 'one_way_latency_us')
            # Add valleys (lowest values)  
            valleys = df.nsmallest(max_points//4, 'one_way_latency_us')
            combined = pd.concat([uniform_sample, peaks, valleys]).drop_duplicates().sort_index()
            return combined.iloc[:max_points] if len(combined) > max_points else combined
        
        elif strategy == 'outlier_preserving':
            # Focus on preserving outliers and anomalies
            q75 = df['one_way_latency_us'].quantile(0.75)
            q25 = df['one_way_latency_us'].quantile(0.25)
            iqr = q75 - q25
            outlier_threshold = q75 + 1.5 * iqr
            
            # Get all outliers
            outliers = df[df['one_way_latency_us'] > outlier_threshold]
            # Get uniform sample of non-outliers
            non_outliers = df[df['one_way_latency_us'] <= outlier_threshold]
            if len(non_outliers) > 0:
                step_size = max(1, len(non_outliers) // (max_points - len(outliers)))
                sampled_normal = non_outliers.iloc[::step_size]
            else:
                sampled_normal = pd.DataFrame()
            
            combined = pd.concat([sampled_normal, outliers]).drop_duplicates().sort_index()
            return combined.iloc[:max_points] if len(combined) > max_points else combined
        
        elif strategy == 'adaptive':
            # Adaptive sampling based on variance
            # More samples in high-variance regions
            df_copy = df.copy().reset_index(drop=True)
            df_copy['variance_window'] = df_copy['one_way_latency_us'].rolling(window=100, min_periods=1).var()
            
            # Sample more from high-variance regions
            high_variance = df_copy[df_copy['variance_window'] > df_copy['variance_window'].median()]
            low_variance = df_copy[df_copy['variance_window'] <= df_copy['variance_window'].median()]
            
            high_var_samples = min(len(high_variance), max_points * 2 // 3)
            low_var_samples = max_points - high_var_samples
            
            if len(high_variance) > 0 and high_var_samples > 0:
                high_step = max(1, len(high_variance) // high_var_samples)
                sampled_high = high_variance.iloc[::high_step]
            else:
                sampled_high = pd.DataFrame()
            
            if len(low_variance) > 0 and low_var_samples > 0:
                low_step = max(1, len(low_variance) // low_var_samples)
                sampled_low = low_variance.iloc[::low_step]
            else:
                sampled_low = pd.DataFrame()
            
            combined = pd.concat([sampled_high, sampled_low]).drop_duplicates().sort_index()
            return combined.iloc[:max_points] if len(combined) > max_points else combined
        
        else:
            # Fallback to uniform
            step_size = len(df) // max_points
            return df.iloc[::step_size].copy()
    
    @staticmethod
    def _get_mechanism_colors(mechanism: str, latency_type: str, available_mechanisms: List = None) -> str:
        """Generate mechanism-specific colors with latency type variations."""
        # Neon/bright colors for maximum visibility on black background
        base_colors = {
            'SharedMemory': '#00FFFF',      # Bright Cyan - highly visible
            'TcpSocket': '#00FF00',         # Bright Green - excellent contrast  
            'UnixDomainSocket': '#FF00FF',  # Bright Magenta - vivid purple
            'PosixMessageQueue': '#FFFF00', # Bright Yellow - high visibility
            'NamedPipe': '#FF0080',         # Hot Pink - vibrant red-pink
            'FileIO': '#0080FF',            # Electric Blue - bright blue
        }
        
        # Get base color or use a default if mechanism not in mapping
        available_mechanisms = available_mechanisms or []
        if mechanism not in base_colors:
            # Bright neon fallback colors for unknown mechanisms
            fallback_colors = ['#FF8000', '#80FF00', '#FF0040', '#00FF80', '#8000FF']
            idx = available_mechanisms.index(mechanism) % len(fallback_colors) if mechanism in available_mechanisms else 0
            base_color = fallback_colors[idx]
        else:
            base_color = base_colors[mechanism]
        
        # Create variations for latency types
        if latency_type == 'one_way':
            return base_color  # Use base color for one-way
        else:  # round_trip
            # Create a darker/more saturated version for round-trip
            # Convert hex to RGB, darken it, convert back
            hex_color = base_color.lstrip('#')
            rgb = tuple(int(hex_color[i:i+2], 16) for i in (0, 2, 4))
            # Darken by reducing values by 25%
            darkened_rgb = tuple(max(0, int(c * 0.75)) for c in rgb)
            return f"#{darkened_rgb[0]:02x}{darkened_rgb[1]:02x}{darkened_rgb[2]:02x}"
    
    @staticmethod 
    def _detect_anomalies(data: pd.DataFrame, mechanism: str, latency_type: str, statistical_overlays: List) -> List:
        """Detect spikes and anomalies in the data."""
        latency_col = 'one_way_latency_us' if latency_type == 'one_way' else 'round_trip_latency_us'
        type_label = 'One-way' if latency_type == 'one_way' else 'Round-trip'
        anomalies = []
        
        # Skip if column doesn't exist in data
        if latency_col not in data.columns or data[latency_col].isna().all():
            return anomalies
        
        if 'spike_detection' in statistical_overlays and len(data) > 20:
            # Simple spike detection using z-score
            try:
                from scipy.stats import zscore
                z_scores = np.abs(zscore(data[latency_col]))
                spike_threshold = 3.0
                spikes = data[z_scores > spike_threshold]
                
                if len(spikes) > 0:
                    anomalies.append({
                        'type': 'spikes',
                        'data': spikes,
                        'description': f"{len(spikes)} latency spikes detected in {mechanism} ({type_label})"
                    })
            except ImportError:
                pass  # Skip spike detection if scipy not available
        
        if 'anomaly_detection' in statistical_overlays and len(data) > 100:
            # Advanced anomaly detection using isolation forest
            try:
                from sklearn.ensemble import IsolationForest
                clf = IsolationForest(contamination=0.1, random_state=42)
                anomaly_labels = clf.fit_predict(data[[latency_col]].values)
                anomaly_data = data[anomaly_labels == -1]
                
                if len(anomaly_data) > 0:
                    anomalies.append({
                        'type': 'anomalies',
                        'data': anomaly_data,
                        'description': f"{len(anomaly_data)} ML anomalies detected in {mechanism} ({type_label})"
                    })
            except Exception:
                # Fallback to simple statistical anomaly detection
                q75 = data[latency_col].quantile(0.75)
                q25 = data[latency_col].quantile(0.25)
                iqr = q75 - q25
                anomaly_threshold = q75 + 2.0 * iqr
                anomaly_data = data[data[latency_col] > anomaly_threshold]
                
                if len(anomaly_data) > 0:
                    anomalies.append({
                        'type': 'statistical_anomalies',
                        'data': anomaly_data,
                        'description': f"{len(anomaly_data)} statistical anomalies in {mechanism} ({type_label})"
                    })
        
        return anomalies
        
        # Placeholder - continuing with original large method for now
        if not percentile_df.empty:
            # Prepare percentile data for display and sort by mechanism performance
            display_percentiles = percentile_df.copy()
            display_percentiles['mechanism'] = pd.Categorical(display_percentiles['mechanism'], categories=mechanism_order, ordered=True)
            display_percentiles = display_percentiles.sort_values(['mechanism', 'message_size', 'latency_type', 'percentile'])
            
            # Round latency values for better display
            display_percentiles['latency_us'] = display_percentiles['latency_us'].round(2)
            
            # Rename columns for better display
            column_mapping = {
                'mechanism': 'Mechanism',
                'message_size': 'Message Size (bytes)',
                'latency_type': 'Latency Type',
                'percentile': 'Percentile',
                'latency_us': 'Latency (μs)',
                'sample_count': 'Sample Count'
            }
            display_percentiles = display_percentiles.rename(columns=column_mapping)
            
            percentile_table = dash_table.DataTable(
                id='percentile-table',
                data=display_percentiles.to_dict('records'),
                columns=[{"name": i, "id": i} for i in display_percentiles.columns],
                style_table={'overflowX': 'auto', 'backgroundColor': '#21262d'},
                style_cell={
                    'textAlign': 'left',
                    'padding': '10px',
                    'fontFamily': 'Inter, sans-serif',
                    'fontSize': '14px',
                    'backgroundColor': '#21262d',
                    'color': '#f0f6fc',
                    'border': '1px solid #30363d'
                },
                style_header={
                    'backgroundColor': '#161b22',
                    'fontWeight': 'bold',
                    'color': '#f0f6fc',
                    'border': '1px solid #30363d'
                },
                style_data_conditional=[
                    {
                        'if': {'row_index': 'odd'},
                        'backgroundColor': '#161b22'
                    }
                ],
                sort_action="native",
                filter_action="native",
                page_action="native",
                page_current=0,
                page_size=10,
            )
            
            percentile_table_div = html.Div([
                html.H3("Dynamic Percentile Statistics"),
                html.P(f"Percentiles calculated directly from {sum(display_percentiles['Sample Count'])//6:,} streaming data points. "
                       f"This provides accurate, real-time percentile calculations based on your selected filters.",
                       style={'color': '#6b7280', 'margin-bottom': '20px', 'fontSize': '0.95rem'}),
                percentile_table
            ], className="dash-table-container")

        # Create separate tables for each section
        latency_table_div = html.Div()
        throughput_table_div = html.Div()
        
        if latency_data:
            # Create 4 separate latency tables: Max (one-way/round-trip) and Average (one-way/round-trip)
            latency_df = pd.DataFrame(latency_data)
            
            # Filter data for each table type
            max_oneway_data = latency_df[(latency_df['type'] == 'Maximum') & (latency_df['source'] == 'One-way')]
            max_roundtrip_data = latency_df[(latency_df['type'] == 'Maximum') & (latency_df['source'] == 'Round-trip')]
            avg_oneway_data = latency_df[(latency_df['type'] == 'Average') & (latency_df['source'] == 'One-way')]
            avg_roundtrip_data = latency_df[(latency_df['type'] == 'Average') & (latency_df['source'] == 'Round-trip')]
            
            # Create pivot tables
            max_oneway_pivot = self.create_pivot_table(max_oneway_data.to_dict('records'), 'latency_us')
            max_roundtrip_pivot = self.create_pivot_table(max_roundtrip_data.to_dict('records'), 'latency_us')
            avg_oneway_pivot = self.create_pivot_table(avg_oneway_data.to_dict('records'), 'latency_us')
            avg_roundtrip_pivot = self.create_pivot_table(avg_roundtrip_data.to_dict('records'), 'latency_us')
            
            # Common table style - Grafana dark theme
            table_style = {
                'style_table': {'overflowX': 'auto', 'width': '100%', 'backgroundColor': '#21262d'},
                'style_cell': {
                    'textAlign': 'center', 
                    'padding': '8px', 
                    'fontFamily': 'Inter, sans-serif', 
                    'fontSize': '12px',
                    'backgroundColor': '#21262d',
                    'color': '#f0f6fc',
                    'border': '1px solid #30363d'
                },
                'style_header': {
                    'backgroundColor': '#161b22', 
                    'fontWeight': 'bold', 
                    'color': '#f0f6fc',
                    'border': '1px solid #30363d'
                },
                'style_data_conditional': [{'if': {'row_index': 'odd'}, 'backgroundColor': '#161b22'}],
                'sort_action': "native"
            }
            
            # Create individual table components
            tables = []
            
            # Max Latency Tables (side by side)
            if not max_oneway_pivot.empty or not max_roundtrip_pivot.empty:
                max_tables_row = html.Div([
                    html.H4("Maximum Latency (μs)", style={'color': '#f85149', 'margin-bottom': '15px', 'text-align': 'center'}),
                    html.Div([
                        # One-way max latency table
                        html.Div([
                            html.H5("One-way", style={'color': '#f0f6fc', 'margin-bottom': '10px', 'text-align': 'center'}),
                            dash_table.DataTable(
                                id='max-oneway-latency-table',
                                data=max_oneway_pivot.to_dict('records') if not max_oneway_pivot.empty else [],
                                columns=[{"name": i, "id": i} for i in max_oneway_pivot.columns] if not max_oneway_pivot.empty else [],
                                **table_style
                            ) if not max_oneway_pivot.empty else html.Div("No data available", style={'text-align': 'center', 'color': '#8b949e'})
                        ], style={'width': '48%', 'display': 'inline-block', 'margin-right': '4%'}),
                        
                        # Round-trip max latency table
                        html.Div([
                            html.H5("Round-trip", style={'color': '#f0f6fc', 'margin-bottom': '10px', 'text-align': 'center'}),
                            dash_table.DataTable(
                                id='max-roundtrip-latency-table',
                                data=max_roundtrip_pivot.to_dict('records') if not max_roundtrip_pivot.empty else [],
                                columns=[{"name": i, "id": i} for i in max_roundtrip_pivot.columns] if not max_roundtrip_pivot.empty else [],
                                **table_style
                            ) if not max_roundtrip_pivot.empty else html.Div("No data available", style={'text-align': 'center', 'color': '#8b949e'})
                        ], style={'width': '48%', 'display': 'inline-block'})
                    ], style={'margin-bottom': '30px'})
                ])
                tables.append(max_tables_row)
            
            # Average Latency Tables (side by side)
            if not avg_oneway_pivot.empty or not avg_roundtrip_pivot.empty:
                avg_tables_row = html.Div([
                    html.H4("Average Latency (μs)", style={'color': '#3fb950', 'margin-bottom': '15px', 'text-align': 'center'}),
                    html.Div([
                        # One-way average latency table
                        html.Div([
                            html.H5("One-way", style={'color': '#f0f6fc', 'margin-bottom': '10px', 'text-align': 'center'}),
                            dash_table.DataTable(
                                id='avg-oneway-latency-table',
                                data=avg_oneway_pivot.to_dict('records') if not avg_oneway_pivot.empty else [],
                                columns=[{"name": i, "id": i} for i in avg_oneway_pivot.columns] if not avg_oneway_pivot.empty else [],
                                **table_style
                            ) if not avg_oneway_pivot.empty else html.Div("No data available", style={'text-align': 'center', 'color': '#8b949e'})
                        ], style={'width': '48%', 'display': 'inline-block', 'margin-right': '4%'}),
                        
                        # Round-trip average latency table
                        html.Div([
                            html.H5("Round-trip", style={'color': '#f0f6fc', 'margin-bottom': '10px', 'text-align': 'center'}),
                            dash_table.DataTable(
                                id='avg-roundtrip-latency-table',
                                data=avg_roundtrip_pivot.to_dict('records') if not avg_roundtrip_pivot.empty else [],
                                columns=[{"name": i, "id": i} for i in avg_roundtrip_pivot.columns] if not avg_roundtrip_pivot.empty else [],
                                **table_style
                            ) if not avg_roundtrip_pivot.empty else html.Div("No data available", style={'text-align': 'center', 'color': '#8b949e'})
                        ], style={'width': '48%', 'display': 'inline-block'})
                    ])
                ])
                tables.append(avg_tables_row)
            
            latency_table_div = html.Div([
                html.H3("Latency Performance Data"),
                html.P("Latency measurements organized by message size and mechanism. Lower values indicate better performance.",
                       style={'color': '#8b949e', 'margin-bottom': '20px', 'fontSize': '0.95rem'}),
                *tables
            ], className="dash-table-container")
        
        if throughput_data:
            # Create 2 separate throughput tables: One-way and Round-trip
            throughput_df = pd.DataFrame(throughput_data)
            
            # Filter data for each table type
            oneway_throughput_data = throughput_df[throughput_df['type'] == 'One-way']
            roundtrip_throughput_data = throughput_df[throughput_df['type'] == 'Round-trip']
            
            # Create pivot tables for messages/sec
            oneway_msgs_pivot = self.create_pivot_table(oneway_throughput_data.to_dict('records'), 'msgs_per_sec', round_digits=1)
            roundtrip_msgs_pivot = self.create_pivot_table(roundtrip_throughput_data.to_dict('records'), 'msgs_per_sec', round_digits=1)
            
            # Create pivot tables for bytes/sec (convert to MB/s for readability)
            oneway_throughput_mb = oneway_throughput_data.copy()
            roundtrip_throughput_mb = roundtrip_throughput_data.copy()
            if not oneway_throughput_mb.empty:
                oneway_throughput_mb['bytes_per_sec'] = oneway_throughput_mb['bytes_per_sec'] / 1_000_000  # Convert to MB/s
            if not roundtrip_throughput_mb.empty:
                roundtrip_throughput_mb['bytes_per_sec'] = roundtrip_throughput_mb['bytes_per_sec'] / 1_000_000  # Convert to MB/s
                
            oneway_bytes_pivot = self.create_pivot_table(oneway_throughput_mb.to_dict('records'), 'bytes_per_sec', round_digits=1)
            roundtrip_bytes_pivot = self.create_pivot_table(roundtrip_throughput_mb.to_dict('records'), 'bytes_per_sec', round_digits=1)
            
            # Common table style for throughput tables - Grafana dark theme
            throughput_table_style = {
                'style_table': {'overflowX': 'auto', 'width': '100%', 'backgroundColor': '#21262d'},
                'style_cell': {
                    'textAlign': 'center', 
                    'padding': '8px', 
                    'fontFamily': 'Inter, sans-serif', 
                    'fontSize': '12px',
                    'backgroundColor': '#21262d',
                    'color': '#f0f6fc',
                    'border': '1px solid #30363d'
                },
                'style_header': {
                    'backgroundColor': '#161b22', 
                    'fontWeight': 'bold', 
                    'color': '#f0f6fc',
                    'border': '1px solid #30363d'
                },
                'style_data_conditional': [{'if': {'row_index': 'odd'}, 'backgroundColor': '#161b22'}],
                'sort_action': "native"
            }
            
            throughput_tables = []
            
            # Messages/sec Tables (side by side)
            if not oneway_msgs_pivot.empty or not roundtrip_msgs_pivot.empty:
                msgs_tables_row = html.Div([
                    html.H4("Message Throughput (messages/sec)", style={'color': '#58a6ff', 'margin-bottom': '15px', 'text-align': 'center'}),
                    html.Div([
                        # One-way messages/sec table
                        html.Div([
                            html.H5("One-way", style={'color': '#f0f6fc', 'margin-bottom': '10px', 'text-align': 'center'}),
                            dash_table.DataTable(
                                id='oneway-msgs-throughput-table',
                                data=oneway_msgs_pivot.to_dict('records') if not oneway_msgs_pivot.empty else [],
                                columns=[{"name": i, "id": i} for i in oneway_msgs_pivot.columns] if not oneway_msgs_pivot.empty else [],
                                **throughput_table_style
                            ) if not oneway_msgs_pivot.empty else html.Div("No data available", style={'text-align': 'center', 'color': '#8b949e'})
                        ], style={'width': '48%', 'display': 'inline-block', 'margin-right': '4%'}),
                        
                        # Round-trip messages/sec table
                        html.Div([
                            html.H5("Round-trip", style={'color': '#f0f6fc', 'margin-bottom': '10px', 'text-align': 'center'}),
                            dash_table.DataTable(
                                id='roundtrip-msgs-throughput-table',
                                data=roundtrip_msgs_pivot.to_dict('records') if not roundtrip_msgs_pivot.empty else [],
                                columns=[{"name": i, "id": i} for i in roundtrip_msgs_pivot.columns] if not roundtrip_msgs_pivot.empty else [],
                                **throughput_table_style
                            ) if not roundtrip_msgs_pivot.empty else html.Div("No data available", style={'text-align': 'center', 'color': '#8b949e'})
                        ], style={'width': '48%', 'display': 'inline-block'})
                    ], style={'margin-bottom': '30px'})
                ])
                throughput_tables.append(msgs_tables_row)
            
            # Bytes/sec Tables (side by side)
            if not oneway_bytes_pivot.empty or not roundtrip_bytes_pivot.empty:
                bytes_tables_row = html.Div([
                    html.H4("Data Throughput (MB/sec)", style={'color': '#58a6ff', 'margin-bottom': '15px', 'text-align': 'center'}),
                    html.Div([
                        # One-way bytes/sec table
                        html.Div([
                            html.H5("One-way", style={'color': '#f0f6fc', 'margin-bottom': '10px', 'text-align': 'center'}),
                            dash_table.DataTable(
                                id='oneway-bytes-throughput-table',
                                data=oneway_bytes_pivot.to_dict('records') if not oneway_bytes_pivot.empty else [],
                                columns=[{"name": i, "id": i} for i in oneway_bytes_pivot.columns] if not oneway_bytes_pivot.empty else [],
                                **throughput_table_style
                            ) if not oneway_bytes_pivot.empty else html.Div("No data available", style={'text-align': 'center', 'color': '#8b949e'})
                        ], style={'width': '48%', 'display': 'inline-block', 'margin-right': '4%'}),
                        
                        # Round-trip bytes/sec table
                        html.Div([
                            html.H5("Round-trip", style={'color': '#f0f6fc', 'margin-bottom': '10px', 'text-align': 'center'}),
                            dash_table.DataTable(
                                id='roundtrip-bytes-throughput-table',
                                data=roundtrip_bytes_pivot.to_dict('records') if not roundtrip_bytes_pivot.empty else [],
                                columns=[{"name": i, "id": i} for i in roundtrip_bytes_pivot.columns] if not roundtrip_bytes_pivot.empty else [],
                                **throughput_table_style
                            ) if not roundtrip_bytes_pivot.empty else html.Div("No data available", style={'text-align': 'center', 'color': '#8b949e'})
                        ], style={'width': '48%', 'display': 'inline-block'})
                    ])
                ])
                throughput_tables.append(bytes_tables_row)
            
            throughput_table_div = html.Div([
                html.H3("Throughput Performance Data"),
                html.P(f"Throughput measurements organized by message size and mechanism. Higher values indicate better performance.",
                       style={'color': '#8b949e', 'margin-bottom': '20px', 'fontSize': '0.95rem'}),
                *throughput_tables
            ], className="dash-table-container")

        # Create performance comparison matrix tables
        comparison_matrix_div = html.Div()
        p50_matrices_exist = not p50_comparison_matrix.empty if isinstance(p50_comparison_matrix, pd.DataFrame) else False
        max_matrices_exist = not max_comparison_matrix.empty if isinstance(max_comparison_matrix, pd.DataFrame) else False
        
        if p50_matrices_exist or max_matrices_exist:
            matrix_tables = []
            
            # P50 Latency Comparison Matrix
            if p50_matrices_exist:
                # Reset index to make mechanism names a column
                p50_display = p50_comparison_matrix.reset_index()
                p50_display.columns.name = None  # Remove column name
                
                # Format the matrix for better display
                p50_display = p50_display.round(1)
                for col in p50_display.columns:
                    if col != 'mechanism' and col != 'Overall Score':
                        p50_display[col] = p50_display[col].apply(lambda x: f"+{x}%" if x != '-' and x > 0 else f"{x}%" if x != '-' else x)
                
                p50_table = dash_table.DataTable(
                    id='p50-comparison-table',
                    data=p50_display.to_dict('records'),
                    columns=[{"name": str(i), "id": str(i)} for i in p50_display.columns],
                    style_table={'overflowX': 'auto', 'backgroundColor': '#21262d'},
                    style_cell={
                        'textAlign': 'center', 
                        'padding': '12px', 
                        'fontFamily': 'Inter, sans-serif', 
                        'fontSize': '14px',
                        'backgroundColor': '#21262d',
                        'color': '#f0f6fc',
                        'border': '1px solid #30363d'
                    },
                    style_header={
                        'backgroundColor': '#161b22',
                        'fontWeight': 'bold',
                        'color': '#58a6ff',
                        'border': '1px solid #30363d'
                    },
                    style_data_conditional=[
                        {'if': {'row_index': 'odd'}, 'backgroundColor': '#161b22'},
                        {'if': {'column_id': 'mechanism'}, 'textAlign': 'left', 'fontWeight': 'bold'},
                        {'if': {'column_id': 'Overall Score'}, 'backgroundColor': '#2d2013', 'fontWeight': 'bold', 'color': '#ffd700'},
                    ],
                    sort_action="native",
                )
                
                matrix_tables.append(html.Div([
                    html.H4("P50 Latency Comparison", style={'color': '#58a6ff', 'margin-bottom': '10px'}),
                    html.P("Relative P50 latency performance. Lower percentages = better typical performance.",
                           style={'color': '#8b949e', 'margin-bottom': '15px', 'fontSize': '0.9rem'}),
                    p50_table
                ], style={'margin-bottom': '30px'}))
            
            # Max Latency Comparison Matrix
            if max_matrices_exist:
                # Reset index to make mechanism names a column
                max_display = max_comparison_matrix.reset_index()
                max_display.columns.name = None  # Remove column name
                
                # Format the matrix for better display
                max_display = max_display.round(1)
                for col in max_display.columns:
                    if col != 'mechanism' and col != 'Overall Score':
                        max_display[col] = max_display[col].apply(lambda x: f"+{x}%" if x != '-' and x > 0 else f"{x}%" if x != '-' else x)
                
                max_table = dash_table.DataTable(
                    id='max-comparison-table',
                    data=max_display.to_dict('records'),
                    columns=[{"name": str(i), "id": str(i)} for i in max_display.columns],
                    style_table={'overflowX': 'auto', 'backgroundColor': '#21262d'},
                    style_cell={
                        'textAlign': 'center',
                        'padding': '12px',
                        'fontFamily': 'Inter, sans-serif',
                        'fontSize': '14px',
                        'backgroundColor': '#21262d',
                        'color': '#f0f6fc',
                        'border': '1px solid #30363d'
                    },
                    style_header={
                        'backgroundColor': '#161b22',
                        'fontWeight': 'bold',
                        'color': '#f85149',
                        'border': '1px solid #30363d'
                    },
                    style_data_conditional=[
                        {'if': {'row_index': 'odd'}, 'backgroundColor': '#161b22'},
                        {'if': {'column_id': 'mechanism'}, 'textAlign': 'left', 'fontWeight': 'bold'},
                        {'if': {'column_id': 'Overall Score'}, 'backgroundColor': '#2d1b1b', 'fontWeight': 'bold', 'color': '#f85149'},
                    ],
                    sort_action="native",
                )
                
                matrix_tables.append(html.Div([
                    html.H4("Max Latency Comparison", style={'color': '#f85149', 'margin-bottom': '10px'}),
                    html.P("Relative maximum latency performance. Lower percentages = better worst-case performance.",
                           style={'color': '#8b949e', 'margin-bottom': '15px', 'fontSize': '0.9rem'}),
                    max_table
                ], style={'margin-bottom': '20px'}))
            
            comparison_matrix_div = html.Div([
                html.H3("Performance Comparison Matrix", style={'color': '#f0f6fc', 'margin-bottom': '15px'}),
                html.P("Comprehensive performance comparison across latency metrics. Lower percentages indicate better performance.",
                       style={'color': '#6b7280', 'margin-bottom': '25px', 'fontSize': '0.95rem'}),
                html.Div(matrix_tables)
            ], className="dash-table-container")

        # Organize layout into logical sections
        return html.Div([
            # === OVERVIEW SECTION ===
            dcc.Loading(
                id="performance-overview-loading",
                type="circle",
                children=html.Div([
                    html.H2("Performance Overview", style={'color': '#f0f6fc', 'margin-bottom': '20px', 'font-size': '1.5rem'}),
                    summary_cards
                ], style={'margin-bottom': '40px'}),
                style={'minHeight': '150px'}
            ),
            
            # === PERFORMANCE COMPARISON SECTION ===
            dcc.Loading(
                id="comparison-matrix-loading",
                type="cube",
                children=html.Div([
                    html.H2("Head-to-Head Comparison", style={'color': '#f0f6fc', 'margin-bottom': '20px', 'font-size': '1.5rem'}),
                    comparison_matrix_div
                ], style={'margin-bottom': '40px'}),
                style={'minHeight': '200px'}
            ),
            
            # === INSIGHTS SECTION ===
            html.Div([
                html.H2("Performance Insights", style={'color': '#f0f6fc', 'margin-bottom': '20px', 'font-size': '1.5rem'}),
                html.Div([
                    html.Div([
                        html.H4("Recommendations:", style={'color': '#3fb950', 'margin-bottom': '15px'}),
                        html.Ul([
                            html.Li(rec, style={'margin-bottom': '8px', 'color': '#f0f6fc'}) 
                            for rec in insights.get('recommendations', ['Select data to see recommendations'])
                        ], style={'padding-left': '20px'})
                    ], style={
                        'background': 'linear-gradient(135deg, #21262d 0%, #161b22 100%)',
                        'padding': '25px', 'border-radius': '12px',
                        'box-shadow': '0 4px 6px rgba(0, 0, 0, 0.3)', 'border': '1px solid #3fb950'
                    })
                ], className="chart-container")
            ], style={'margin-bottom': '40px'}),
            
            # === LATENCY ANALYSIS SECTION ===
            dcc.Loading(
                id="latency-analysis-loading",
                type="default",
                children=html.Div([
                    html.H2("Latency Analysis", style={'color': '#f0f6fc', 'margin-bottom': '20px', 'font-size': '1.5rem'}),
                html.Div([
                    dcc.Graph(id='avg-latency-chart', figure=self.apply_dark_theme(avg_latency_fig))
            ], className="chart-container"),
                html.Div([
                    dcc.Graph(id='max-latency-chart', figure=self.apply_dark_theme(max_latency_fig))
                ], className="chart-container"),
                    latency_table_div if latency_data else html.Div()
                ], style={'margin-bottom': '40px'}),
                style={'minHeight': '300px'}
            ),
            
            # === THROUGHPUT ANALYSIS SECTION ===
            dcc.Loading(
                id="throughput-analysis-loading",
                type="default",
                children=html.Div([
                    html.H2("Throughput Analysis", style={'color': '#f0f6fc', 'margin-bottom': '20px', 'font-size': '1.5rem'}),
            html.Div([
                dcc.Graph(id='throughput-msgs-chart', figure=self.apply_dark_theme(msgs_fig))
            ], className="chart-container"),
            html.Div([
                dcc.Graph(id='throughput-bytes-chart', figure=self.apply_dark_theme(bytes_fig))
            ], className="chart-container"),
                    throughput_table_div if throughput_data else html.Div()
                ], style={'margin-bottom': '40px'}),
                style={'minHeight': '300px'}
            )
        ])
    
    @safe_computation(default_return=html.Div("Error rendering summary"))
    def render_summary_tab_cached(self, mechanisms: List, message_sizes: List):
        """Cached version of render_summary_tab with enhanced error handling and threading."""
        return self._render_with_threading(self.render_summary_tab, mechanisms, message_sizes, "Summary")
    
    @safe_computation(default_return=html.Div("Error rendering time series"))
    def render_timeseries_tab_cached(self, mechanisms: List, message_sizes: List):
        """Cached version of render_timeseries_tab with enhanced error handling and threading."""
        return self._render_with_threading(self.render_timeseries_tab, mechanisms, message_sizes, "Time Series")
    
    def _render_with_threading(self, render_func, mechanisms, message_sizes, tab_name):
        """Helper method to render tabs with threading and progress indication."""
        try:
            logger.info(f"Starting {tab_name} analysis with threading...")
            start_time = time.time()
            
            # Use threading for rendering
            with ThreadPoolExecutor(max_workers=2) as executor:
                future = executor.submit(render_func, mechanisms, message_sizes)
                result = future.result(timeout=120)  # 2 minute timeout
            
            computation_time = time.time() - start_time
            logger.info(f"{tab_name} analysis completed in {computation_time:.2f}s")
            
            return result
            
        except Exception as e:
            logger.error(f"Error in {tab_name} rendering: {str(e)}")
            return html.Div([
                html.Div([
                    html.H3(f"{tab_name} Analysis Error", style={'color': '#f85149', 'margin-bottom': '15px'}),
                    html.P(f"An error occurred during {tab_name.lower()} analysis:", style={'color': '#f0f6fc', 'margin-bottom': '10px'}),
                    html.Code(str(e), style={'background-color': '#fee2e2', 'padding': '8px', 'border-radius': '4px', 'color': '#991b1b'}),
                    html.P("Please check your data and filters, then try again.", style={'color': '#6b7280', 'margin-top': '15px', 'font-style': 'italic'})
                ], style={
                    'text-align': 'center', 
                    'padding': '40px',
                    'background-color': '#fefefe',
                    'border': '1px solid #fecaca',
                    'border-radius': '8px',
                    'margin': '20px'
                })
            ])
    
    @safe_computation(default_return={})
    def _create_charts_threaded(self, chart_functions):
        """Create multiple charts using threading for better performance."""
        try:
            logger.info(f"Creating {len(chart_functions)} charts with threading...")
            start_time = time.time()
            
            # Use threading for chart generation
            with ThreadPoolExecutor(max_workers=4) as executor:
                # Submit all chart creation tasks
                futures = {executor.submit(func): name for func, name in chart_functions}
                
                results = {}
                for future in as_completed(futures):
                    chart_name = futures[future]
                    try:
                        results[chart_name] = future.result(timeout=60)  # 1 minute per chart
                        logger.debug(f"Chart '{chart_name}' created successfully")
                    except Exception as e:
                        logger.error(f"Error creating chart '{chart_name}': {str(e)}")
                        results[chart_name] = self._create_error_chart(f"Error in {chart_name}: {str(e)}")
            
            computation_time = time.time() - start_time
            logger.info(f"Chart generation completed in {computation_time:.2f}s")
            
            return results
            
        except Exception as e:
            logger.error(f"Error in threaded chart creation: {str(e)}")
            return {}
    
    @safe_computation(default_return=go.Figure())
    def _create_error_chart(self, error_message):
        """Create an error chart for display when chart generation fails."""
        fig = go.Figure().add_annotation(
            text=f"Error: {error_message}",
            x=0.5, y=0.5, xref="paper", yref="paper", showarrow=False,
            font=dict(size=14, color="#f85149")
        )
        return self.apply_dark_theme(fig)
    
    def render_timeseries_tab(self, mechanisms: List, message_sizes: List):
        """Render time series analysis tab."""
        streaming_df = self.filter_data(self.data_store['streaming_data'], mechanisms, message_sizes)
        
        # Create time series controls section (always show controls)
        available_mechanisms = mechanisms if mechanisms and len(mechanisms) > 0 else (
            streaming_df['mechanism'].unique().tolist() if not streaming_df.empty else ['PosixMessageQueue', 'SharedMemory', 'UnixDomainSocket']
        )
        
        controls_section = html.Div([
            html.H3("Time Series Controls", style={'margin-bottom': '20px'}),
            
            # Quick Preset Configuration Buttons
            html.Div([
                html.Label("Quick Presets:", style={'margin-bottom': '10px', 'display': 'block', 'font-weight': 'bold'}),
                html.Div([
                    html.Button("Performance Analysis", id="preset-performance", n_clicks=0, 
                              className="preset-btn", style={'margin-right': '10px', 'margin-bottom': '8px'},
                              title="Separate charts with performance zones and percentile bands for overall performance analysis"),
                    html.Button("Detailed Inspection", id="preset-detailed", n_clicks=0, 
                              className="preset-btn", style={'margin-right': '10px', 'margin-bottom': '8px'},
                              title="Separate charts with spike detection and distribution overlays for detailed mechanism analysis"),
                    html.Button("Statistical Overview", id="preset-statistical", n_clicks=0, 
                              className="preset-btn", style={'margin-right': '10px', 'margin-bottom': '8px'},
                              title="Faceted view by message size with rolling percentiles and statistical overlays"),
                    html.Button("Outlier Detection", id="preset-outliers", n_clicks=0, 
                              className="preset-btn", style={'margin-right': '10px', 'margin-bottom': '8px'},
                              title="Separate charts with spike and anomaly detection for identifying outliers"),
                    html.Button("Reset to Default", id="preset-reset", n_clicks=0, 
                              className="preset-btn-secondary", style={'margin-bottom': '8px'},
                              title="Reset all controls to default settings (preserves statistics panel state)"),
                ]),
            ], style={'margin-bottom': '20px', 'padding': '15px', 'background': 'linear-gradient(135deg, #21262d 0%, #161b22 100%)', 'border-radius': '8px', 'border': '1px solid #30363d'}),
            
            # Chart Layout & Statistical Controls
            html.Div([
                html.Label("Chart Layout & Statistical Overlays:", style={'margin-bottom': '15px', 'display': 'block', 'font-weight': 'bold', 'font-size': '1.1em', 'color': '#f0f6fc'}),
                html.Div([
                    # Chart Layout
                    html.Div([
                        html.Label("Chart Layout:", style={'margin-bottom': '10px', 'display': 'block', 'font-weight': 'bold', 'color': '#f0f6fc'}),
                        html.Div([
                            dcc.RadioItems(
                                id='ts-chart-layout',
                                options=[
                                    {'label': 'Separate Charts (by mechanism)', 'value': 'separate'},
                                    {'label': 'Separate Charts (by message size)', 'value': 'faceted'}
                                ],
                                value='separate',
                                className="chart-layout-radio",
                                style={'margin-bottom': '15px'}
                            ),
                        ], title="Choose how to organize charts: separate charts for each mechanism, or separate charts for each message size"),
                        
                        # Statistical Overlays in same column
                        html.Label("Statistical Overlays:", style={'margin-bottom': '10px', 'display': 'block', 'font-weight': 'bold', 'color': '#f0f6fc'}),
                        html.Div([
                            dcc.Checklist(
                                id='ts-statistical-overlays',
                                options=[
                                    {'label': 'Percentile Bands (P25-P75)', 'value': 'percentile_bands'},
                                    {'label': 'Spike Detection (Z-score outliers)', 'value': 'spike_detection'},
                                    {'label': 'Anomaly Detection (ML pattern analysis)', 'value': 'anomaly_detection'},
                                    {'label': 'Distribution Overlays', 'value': 'distributions'},
                                    {'label': 'Rolling P95/P99', 'value': 'rolling_percentiles'}
                                ],
                                value=['percentile_bands'],
                                className="filter-checklist"
                            ),
                        ], title="Statistical overlays: confidence bands, spike/anomaly detection, mini histogram overlays, rolling percentile calculations"),
                    ], style={'width': '48%', 'display': 'inline-block', 'vertical-align': 'top'}),
                    
                    # View Options and Latency Type
                    html.Div([
                        html.Label("View Options:", style={'margin-bottom': '10px', 'display': 'block', 'font-weight': 'bold', 'color': '#f0f6fc'}),
                        html.Div([
                            dcc.Checklist(
                                id='ts-view-options',
                                options=[
                                    {'label': 'Sync Zoom/Pan', 'value': 'sync_zoom'},
                                    {'label': 'Show Statistics Panel', 'value': 'show_stats'},
                                    {'label': 'Enable Crossfilter', 'value': 'crossfilter'},
                                    {'label': 'Performance Zones', 'value': 'perf_zones'}
                                ],
                                value=['sync_zoom'],
                                className="filter-checklist"
                            ),
                        ], title="View options: sync zoom/pan across charts, show detailed statistics, enable interactive filtering, highlight performance zones"),
                        
                        html.Br(),
                        html.Label("Latency Type:", style={'margin-bottom': '10px', 'display': 'block', 'font-weight': 'bold', 'color': '#f0f6fc'}),
                        html.Div([
                            dcc.Checklist(
                                id='ts-latency-types',
                                options=[
                                    {'label': 'One-way Latency', 'value': 'one_way'},
                                    {'label': 'Round-trip Latency', 'value': 'round_trip'}
                                ],
                                value=['one_way'],
                                className="filter-checklist"
                            ),
                        ], title="Select which latency types to display in charts and statistics: one-way (send only) or round-trip (send + receive)"),
                    ], style={'width': '48%', 'display': 'inline-block', 'vertical-align': 'top', 'margin-left': '4%'}),
                ])
            ], style={'margin-bottom': '30px', 'padding': '15px', 'background': 'linear-gradient(135deg, #21262d 0%, #161b22 100%)', 'border-radius': '8px', 'border': '1px solid #30363d'}),
            
            html.Div([
                html.Div([
                    html.Label("Moving Average Window:", style={'margin-bottom': '10px', 'display': 'block'}),
                    dcc.Slider(
                        id='ts-moving-avg-slider',
                        min=1,
                        max=1000,
                        step=1,
                        value=100,
                        marks={i: str(i) for i in [1, 100, 500, 1000]},
                        tooltip={"placement": "bottom", "always_visible": True}
                    ),
                ], style={'width': '30%', 'display': 'inline-block', 'vertical-align': 'top'}),
                
                html.Div([
                    html.Label("Display Points:", style={'margin-bottom': '10px', 'display': 'block'}),
                    dcc.Slider(
                        id='ts-display-points-slider',
                        min=1000,
                        max=50000,
                        step=1000,
                        value=10000,
                        marks={i: f'{i//1000}K' for i in [1000, 5000, 10000, 25000, 50000]},
                        tooltip={"placement": "bottom", "always_visible": True}
                    ),
                ], style={'width': '30%', 'display': 'inline-block', 'vertical-align': 'top', 'margin-left': '5%'}),
                
                html.Div([
                    html.Label("Display Options:", style={'margin-bottom': '10px', 'display': 'block'}),
                    dcc.Checklist(
                        id='ts-display-options',
                        options=[
                            {'label': 'Show Raw Data Points', 'value': 'raw_dots'},
                            {'label': 'Show Moving Average', 'value': 'moving_avg'}
                        ],
                        value=['raw_dots', 'moving_avg'],
                        className="filter-checklist"
                    ),
                ], style={'width': '25%', 'display': 'inline-block', 'vertical-align': 'top', 'margin-left': '3%'}),
                
                html.Div([
                    html.Label("Y-Axis Options:", style={'margin-bottom': '10px', 'display': 'block', 'font-weight': 'bold'}),
                    html.Div([
                        dcc.RadioItems(
                            id='ts-y-axis-scale',
                            options=[
                                {'label': 'Linear Scale', 'value': 'linear'},
                                {'label': 'Log Scale', 'value': 'log'}
                            ],
                            value='log',
                            className="axis-scale-radio"
                        ),
                    ], title="Y-axis scaling: linear for normal data, logarithmic for wide value ranges or highlighting multiplicative differences"),
                    html.Br(),
                    html.Label("Range:", style={'margin-bottom': '5px', 'display': 'block', 'font-size': '0.9rem'}),
                    dcc.RadioItems(
                        id='ts-y-axis-range',
                        options=[
                            {'label': 'Auto', 'value': 'auto'},
                            {'label': 'Fixed', 'value': 'fixed'}
                        ],
                        value='auto',
                        className="axis-range-radio"
                    ),
                ], style={'width': '30%', 'display': 'inline-block', 'vertical-align': 'top', 'margin-left': '3%'}),
            
            ], style={'margin-bottom': '30px'}),
            
            # Sampling Controls
            html.Div([
                
                html.Div([
                    html.Label("Sampling Strategy:", style={'margin-bottom': '10px', 'display': 'block', 'font-weight': 'bold'}),
                    html.Div([
                        dcc.Dropdown(
                            id='ts-sampling-strategy',
                            options=[
                                {'label': 'Uniform Sampling', 'value': 'uniform'},
                                {'label': 'Peak-Preserving', 'value': 'peak_preserving'},
                                {'label': 'Outlier-Preserving', 'value': 'outlier_preserving'},
                                {'label': 'Adaptive Sampling', 'value': 'adaptive'}
                            ],
                            value='uniform',
                            className="sampling-dropdown"
                        ),
                    ], title="Data sampling for large datasets: uniform (even spacing), peak-preserving (maintains peaks), outlier-preserving (keeps extreme values), adaptive (smart density-based)"),
                ], style={'width': '30%', 'display': 'inline-block', 'vertical-align': 'top', 'margin-left': '3%'}),
                

            ], style={'margin-bottom': '30px'}),
            

            
            # Charts container that will be updated
            dcc.Loading(
                id="ts-charts-loading",
                type="default",
                children=html.Div(id='ts-charts-container'),
                style={'minHeight': '400px'}
            ),
            
            # Real-time Statistics Panel (conditional)
            dcc.Loading(
                id="ts-stats-loading",
                type="dot",
                children=html.Div(id='ts-stats-panel', style={'margin-bottom': '20px'}),
                style={'minHeight': '50px'}
            )
        ], className="filter-section", style={'margin-bottom': '30px'})
        
        # Check if we have data to show a helpful message
        if streaming_df.empty:
            no_data_message = html.Div([
                html.Div([
                    html.H3("Time Series Analysis Ready", style={'color': '#f0f6fc', 'margin-bottom': '20px'}),
                    html.P("Configure your time series analysis settings above and click 'Run Analysis' to visualize latency patterns over time.", 
                           style={'color': '#8b949e', 'font-size': '1.1rem', 'line-height': '1.6', 'margin-bottom': '20px'}),
                    html.Div([
                        html.H4("Available Analysis Features:", style={'color': '#f0f6fc', 'margin-bottom': '15px'}),
                        html.Ul([
                            html.Li("Interactive latency trends visualization"),
                            html.Li("Statistical overlays and anomaly detection"),
                            html.Li("Performance zones and percentile bands"),
                            html.Li("Flexible chart layouts and sampling options"),
                            html.Li("Real-time statistics and distribution analysis")
                        ], style={'color': '#f0f6fc', 'padding-left': '20px', 'line-height': '1.8'})
                    ])
                ], style={
                    'text-align': 'center', 
                    'padding': '40px 30px',
                    'background': 'linear-gradient(135deg, #f8fafc 0%, #e2e8f0 100%)',
                    'border-radius': '12px',
                    'margin': '20px 0',
                    'border': '2px dashed #cbd5e1'
                })
            ], style={'margin-top': '30px'})
            
            return html.Div([controls_section, no_data_message])
        
        return controls_section
    
    def render_timeseries_charts(self, mechanisms: List, message_sizes: List, 
                                moving_avg_window: int, display_points: int, display_options: List,
                                chart_layout: str = 'separate',
                                view_options: List = None, latency_types: List = None, statistical_overlays: List = None, 
                                y_axis_scale: str = 'log', 
                                y_axis_range: str = 'auto', sampling_strategy: str = 'uniform'):
        """Render the actual time series charts with advanced features and multiple layout options."""
        # Initialize default parameters
        view_options = view_options or ['sync_zoom']
        latency_types = latency_types or ['one_way']
        statistical_overlays = statistical_overlays or ['percentile_bands']
        
        streaming_df = self.filter_data(self.data_store['streaming_data'], mechanisms, message_sizes)
        
        if streaming_df.empty:
            return html.Div([
                html.P("No data available for current selections.", 
                       style={'textAlign': 'center', 'color': '#6b7280', 'fontSize': '1.1rem', 'padding': '40px'})
            ])

        # Advanced sampling strategies - use display_points from slider
        MAX_POINTS_PER_RUN = display_points
        total_original_points = len(streaming_df)
        performance_note = html.Div()
        
        def apply_sampling_strategy(df, strategy, max_points):
            """Apply different sampling strategies to the dataframe."""
            if len(df) <= max_points:
                return df
                
            if strategy == 'uniform':
                # Standard uniform sampling
                step_size = len(df) // max_points
                return df.iloc[::step_size].copy()
            
            elif strategy == 'peak_preserving':
                # Preserve peaks and valleys in the data
                uniform_sample = df.iloc[::len(df)//max_points].copy()
                # Add peaks (highest values)
                peaks = df.nlargest(max_points//4, 'one_way_latency_us')
                # Add valleys (lowest values)  
                valleys = df.nsmallest(max_points//4, 'one_way_latency_us')
                combined = pd.concat([uniform_sample, peaks, valleys]).drop_duplicates().sort_index()
                return combined.iloc[:max_points] if len(combined) > max_points else combined
            
            elif strategy == 'outlier_preserving':
                # Focus on preserving outliers and anomalies
                q75 = df['one_way_latency_us'].quantile(0.75)
                q25 = df['one_way_latency_us'].quantile(0.25)
                iqr = q75 - q25
                outlier_threshold = q75 + 1.5 * iqr
                
                # Get all outliers
                outliers = df[df['one_way_latency_us'] > outlier_threshold]
                # Get uniform sample of non-outliers
                non_outliers = df[df['one_way_latency_us'] <= outlier_threshold]
                if len(non_outliers) > 0:
                    step_size = max(1, len(non_outliers) // (max_points - len(outliers)))
                    sampled_normal = non_outliers.iloc[::step_size]
                else:
                    sampled_normal = pd.DataFrame()
                
                combined = pd.concat([sampled_normal, outliers]).drop_duplicates().sort_index()
                return combined.iloc[:max_points] if len(combined) > max_points else combined
            
            elif strategy == 'adaptive':
                # Adaptive sampling based on variance
                # More samples in high-variance regions
                df_copy = df.copy().reset_index(drop=True)
                df_copy['variance_window'] = df_copy['one_way_latency_us'].rolling(window=100, min_periods=1).var()
                
                # Sample more from high-variance regions
                high_variance = df_copy[df_copy['variance_window'] > df_copy['variance_window'].median()]
                low_variance = df_copy[df_copy['variance_window'] <= df_copy['variance_window'].median()]
                
                high_var_samples = min(len(high_variance), max_points * 2 // 3)
                low_var_samples = max_points - high_var_samples
                
                if len(high_variance) > 0 and high_var_samples > 0:
                    high_step = max(1, len(high_variance) // high_var_samples)
                    sampled_high = high_variance.iloc[::high_step]
                else:
                    sampled_high = pd.DataFrame()
                
                if len(low_variance) > 0 and low_var_samples > 0:
                    low_step = max(1, len(low_variance) // low_var_samples)
                    sampled_low = low_variance.iloc[::low_step]
                else:
                    sampled_low = pd.DataFrame()
                
                combined = pd.concat([sampled_high, sampled_low]).drop_duplicates().sort_index()
                return combined.iloc[:max_points] if len(combined) > max_points else combined
            
            else:
                # Fallback to uniform
                step_size = len(df) // max_points
                return df.iloc[::step_size].copy()
        
        if total_original_points > MAX_POINTS_PER_RUN:
            strategy_info = {
                'uniform': {
                    'name': 'Uniform Sampling',
                    'definition': 'Takes every Nth data point at regular intervals, like picking every 10th sample.',
                    'pros': 'Fast and consistent; gives balanced view across time',
                    'cons': 'May miss important spikes or rare events between samples'
                },
                'peak_preserving': {
                    'name': 'Peak-Preserving Sampling', 
                    'definition': 'Combines regular sampling with the highest and lowest values to keep important extremes.',
                    'pros': 'Captures performance spikes and dips; good for identifying bottlenecks',
                    'cons': 'Slightly slower; may over-emphasize outliers in analysis'
                },
                'outlier_preserving': {
                    'name': 'Outlier-Preserving Sampling',
                    'definition': 'Focuses on keeping unusual values (outliers) while sampling normally elsewhere.',
                    'pros': 'Excellent for anomaly detection and debugging unusual behavior',
                    'cons': 'May skew statistical analysis; less representative of typical performance'
                },
                'adaptive': {
                    'name': 'Adaptive Variance-Based Sampling',
                    'definition': 'Smart sampling that keeps more data points where values change rapidly.',
                    'pros': 'Best balance of performance and detail; adapts to data complexity',
                    'cons': 'Most computationally expensive; may be overkill for simple datasets'
                }
            }
            
            current_strategy = strategy_info.get(sampling_strategy, strategy_info['uniform'])
            
            performance_note = html.Div([
                html.P(f"Performance Note: Using {current_strategy['name']} for better interactivity. "
                       f"Original: {total_original_points:,} points, "
                       f"Displaying: ~{MAX_POINTS_PER_RUN:,} points.", 
                       style={'margin-bottom': '8px'}),
                html.P([
                    html.Strong("What it does: "), 
                    current_strategy['definition']
                ], style={'font-size': '0.85rem', 'margin-bottom': '5px'}),
                html.P([
                    html.Strong("Pros: ", style={'color': '#f0f6fc'}), 
                    current_strategy['pros']
                ], style={'font-size': '0.85rem', 'margin-bottom': '3px', 'color': '#f0f6fc'}),
                html.P([
                    html.Strong("Cons: ", style={'color': '#f0f6fc'}), 
                    current_strategy['cons']
                ], style={'font-size': '0.85rem', 'color': '#f0f6fc'})
            ], className="performance-note")

        # Advanced Chart Generation with Multiple Layout Options
        # Mechanism-specific color scheme with latency type variations
        def get_mechanism_colors(mechanism, latency_type):
            """Generate mechanism-specific colors with latency type variations."""
            # Neon/bright colors for maximum visibility on black background
            base_colors = {
                'SharedMemory': '#00FFFF',      # Bright Cyan - highly visible
                'TcpSocket': '#00FF00',         # Bright Green - excellent contrast  
                'UnixDomainSocket': '#FF00FF',  # Bright Magenta - vivid purple
                'PosixMessageQueue': '#FFFF00', # Bright Yellow - high visibility
                'NamedPipe': '#FF0080',         # Hot Pink - vibrant red-pink
                'FileIO': '#0080FF',            # Electric Blue - bright blue
            }
            
            # Get base color or use a default if mechanism not in mapping
            available_mechanisms = list(streaming_df['mechanism'].unique()) if len(streaming_df) > 0 else []
            if mechanism not in base_colors:
                # Bright neon fallback colors for unknown mechanisms
                fallback_colors = ['#FF8000', '#80FF00', '#FF0040', '#00FF80', '#8000FF']
                idx = available_mechanisms.index(mechanism) % len(fallback_colors) if mechanism in available_mechanisms else 0
                base_color = fallback_colors[idx]
            else:
                base_color = base_colors[mechanism]
            
            # Create variations for latency types
            if latency_type == 'one_way':
                return base_color  # Use base color for one-way
            else:  # round_trip
                # Create a darker/more saturated version for round-trip
                # Convert hex to RGB, darken it, convert back
                hex_color = base_color.lstrip('#')
                rgb = tuple(int(hex_color[i:i+2], 16) for i in (0, 2, 4))
                # Darken by reducing values by 20%
                darkened_rgb = tuple(max(0, int(c * 0.75)) for c in rgb)
                return f"#{darkened_rgb[0]:02x}{darkened_rgb[1]:02x}{darkened_rgb[2]:02x}"
        
        # Symbol scheme: different symbols for each mechanism
        symbols = ['circle', 'square', 'diamond', 'cross', 'x', 'triangle-up', 'triangle-down', 'star']
        mechanism_colors = ['#00FFFF', '#FF0080', '#00FF00', '#FFFF00', '#FF00FF', '#FF8000', '#0080FF', '#80FF00']
        
        charts = []
        stats_panel = html.Div()
        
        # Generate statistics panel if requested
        if 'show_stats' in view_options:
            stats_data = []
            for mechanism in streaming_df['mechanism'].unique():
                mech_data = streaming_df[streaming_df['mechanism'] == mechanism]
                if len(mech_data) > 0:
                    stats_data.append({
                        'Mechanism': mechanism,
                        'Count': f"{len(mech_data):,}",
                        'Mean (μs)': f"{mech_data['one_way_latency_us'].mean():.2f}",
                        'Std (μs)': f"{mech_data['one_way_latency_us'].std():.2f}",
                        'Min (μs)': f"{mech_data['one_way_latency_us'].min():.2f}",
                        'Max (μs)': f"{mech_data['one_way_latency_us'].max():.2f}",
                        'P95 (μs)': f"{mech_data['one_way_latency_us'].quantile(0.95):.2f}",
                        'P99 (μs)': f"{mech_data['one_way_latency_us'].quantile(0.99):.2f}"
                    })
            
            if stats_data:
                stats_panel = html.Div([
                    html.H4("Real-time Statistics", style={'margin-bottom': '15px', 'color': '#1f2937'}),
                    dash_table.DataTable(
                        data=stats_data,
                        columns=[{"name": col, "id": col} for col in stats_data[0].keys()],
                        style_cell={'textAlign': 'left', 'padding': '10px', 'font-size': '0.85rem'},
                        style_header={'backgroundColor': '#f8fafc', 'fontWeight': 'bold', 'border': '1px solid #e2e8f0'},
                        style_data={'backgroundColor': '#ffffff', 'border': '1px solid #e2e8f0'},
                        style_table={'border-radius': '8px', 'overflow': 'hidden'},
                    )
                ], style={'margin-bottom': '30px', 'padding': '20px', 'background': '#f8fafc', 'border-radius': '8px', 'border': '1px solid #e2e8f0'})
        
        # Helper function to add statistical overlays
        def add_statistical_overlays(fig, data, mechanism, color, latency_type='one_way'):
            """Add percentile bands and other statistical overlays."""
            latency_col = 'one_way_latency_us' if latency_type == 'one_way' else 'round_trip_latency_us'
            type_label = 'One-way' if latency_type == 'one_way' else 'Round-trip'
            
            # Skip if column doesn't exist in data
            if latency_col not in data.columns or data[latency_col].isna().all():
                return
                
            # Add visual overlays for spikes and anomalies
            if ('spike_detection' in statistical_overlays or 'anomaly_detection' in statistical_overlays) and len(data) > 20:
                anomalies = self._detect_anomalies(data, mechanism, latency_type, statistical_overlays)
                
                for anomaly in anomalies:
                    anomaly_data = anomaly['data']
                    if len(anomaly_data) > 0:
                        if anomaly['type'] == 'spikes':
                            # Add spike markers (using magenta to avoid conflicts with red mechanism colors)
                            fig.add_trace(go.Scatter(
                                x=anomaly_data.index,
                                y=anomaly_data[latency_col],
                                mode='markers',
                                name=f'Spikes ({mechanism} {type_label})',
                                marker=dict(
                                    color='#ff6b9d',  # Brighter pink/magenta - more visible
                                    size=10,
                                    symbol='triangle-up',
                                    line=dict(width=2, color='#ff1744')  # Bright red border for high contrast
                                ),
                                showlegend=True,
                                hovertemplate=f'<b>Spike Detected</b><br>%{{x}}<br>%{{y:.2f}} μs<extra></extra>'
                            ))
                        else:  # anomalies or statistical_anomalies
                            # Add anomaly markers (using brown to avoid conflicts with all mechanism colors)
                            fig.add_trace(go.Scatter(
                                x=anomaly_data.index,
                                y=anomaly_data[latency_col],
                                mode='markers',
                                name=f'Anomalies ({mechanism} {type_label})',
                                marker=dict(
                                    color='#a16207',  # Brown - distinct from all mechanism colors
                                    size=6,
                                    symbol='diamond',
                                    line=dict(width=1, color='#713f12')  # Dark brown border
                                ),
                                showlegend=True,
                                hovertemplate=f'<b>Anomaly Detected</b><br>%{{x}}<br>%{{y:.2f}} μs<extra></extra>'
                            ))
                
            if 'percentile_bands' in statistical_overlays and len(data) > 50:
                # Add P25-P75 confidence bands
                data_sorted = data.sort_index()
                window_size = min(100, len(data) // 10)
                
                if window_size > 1:
                    p25_rolling = data_sorted[latency_col].rolling(window=window_size, min_periods=1).quantile(0.25)
                    p75_rolling = data_sorted[latency_col].rolling(window=window_size, min_periods=1).quantile(0.75)
                    
                    # Add percentile band
                    fig.add_trace(go.Scatter(
                        x=data_sorted.index,
                        y=p75_rolling,
                        mode='lines',
                        name=f'P75 ({mechanism} {type_label})',
                        line=dict(color=color, width=1, dash='dot'),
                        opacity=0.4,
                        showlegend=False,
                        hoverinfo='skip'
                    ))
                    
                    # Convert color to rgba for fill
                    try:
                        import matplotlib.colors as mcolors
                        rgb = mcolors.to_rgb(color)
                        fillcolor = f'rgba({int(rgb[0]*255)}, {int(rgb[1]*255)}, {int(rgb[2]*255)}, 0.15)'
                    except:
                        fillcolor = 'rgba(59, 130, 246, 0.15)'  # fallback blue
                    
                    fig.add_trace(go.Scatter(
                        x=data_sorted.index,
                        y=p25_rolling,
                        mode='lines',
                        name=f'P25-P75 Band ({mechanism} {type_label})',
                        line=dict(color=color, width=1, dash='dot'),
                        fill='tonexty',
                        fillcolor=fillcolor,
                        opacity=0.4,
                        showlegend=True,
                        hoverinfo='skip',
                        hovertemplate=f'<b>Percentile Band</b><br>P25: %{{y:.2f}} μs<br>Index: %{{x}}<extra></extra>'
                    ))
            
            if 'rolling_percentiles' in statistical_overlays and len(data) > 100:
                # Add rolling P95 and P99 percentiles
                data_sorted = data.sort_index()
                window_size = min(200, len(data) // 5)
                
                if window_size > 10:
                    p95_rolling = data_sorted[latency_col].rolling(window=window_size, min_periods=10).quantile(0.95)
                    p99_rolling = data_sorted[latency_col].rolling(window=window_size, min_periods=10).quantile(0.99)
                    
                    # Add P95 line
                    fig.add_trace(go.Scatter(
                        x=data_sorted.index,
                        y=p95_rolling,
                        mode='lines',
                        name=f'P95 Rolling ({mechanism} {type_label})',
                        line=dict(color=color, width=2, dash='dashdot'),
                        opacity=0.8,
                        showlegend=True,
                        hovertemplate=f'<b>P95 Rolling</b><br>Value: %{{y:.2f}} μs<br>Index: %{{x}}<extra></extra>'
                    ))
                    
                    # Add P99 line
                    fig.add_trace(go.Scatter(
                        x=data_sorted.index,
                        y=p99_rolling,
                        mode='lines',
                        name=f'P99 Rolling ({mechanism} {type_label})',
                        line=dict(color=color, width=2, dash='longdash'),
                        opacity=0.8,
                        showlegend=True,
                        hovertemplate=f'<b>P99 Rolling</b><br>Value: %{{y:.2f}} μs<br>Index: %{{x}}<extra></extra>'
                    ))
            
            if 'distributions' in statistical_overlays and len(data) > 50:
                # Add distribution overlay as a small histogram in upper right corner
                latency_values = data[latency_col].dropna()
                
                if len(latency_values) > 20:
                    # Calculate histogram
                    hist_counts, hist_bins = np.histogram(latency_values, bins=20)
                    hist_centers = (hist_bins[:-1] + hist_bins[1:]) / 2
                    
                    # Normalize counts to fit in a small area (scale to 10% of y-range)
                    y_range = fig.layout.yaxis.range if fig.layout.yaxis.range else [latency_values.min(), latency_values.max()]
                    if not y_range or y_range[0] == y_range[1]:
                        y_range = [latency_values.min(), latency_values.max()]
                    
                    y_span = y_range[1] - y_range[0]
                    normalized_counts = (hist_counts / hist_counts.max()) * (y_span * 0.1) + y_range[1] * 0.85
                    
                    # Add histogram bars as scatter plot
                    fig.add_trace(go.Scatter(
                        x=normalized_counts,
                        y=hist_centers,
                            mode='lines',
                        name=f'Distribution ({mechanism} {type_label})',
                        line=dict(color=color, width=2),
                        opacity=0.6,
                        showlegend=True,
                        hovertemplate=f'<b>Distribution</b><br>Latency: %{{y:.2f}} μs<br>Frequency: %{{x:.0f}}<extra></extra>',
                        xaxis='x2',
                        yaxis='y'
                    ))

        
        # Helper function to detect spikes and anomalies
        def detect_anomalies(data, mechanism, latency_type='one_way'):
            """Detect spikes and anomalies in the data."""
            latency_col = 'one_way_latency_us' if latency_type == 'one_way' else 'round_trip_latency_us'
            type_label = 'One-way' if latency_type == 'one_way' else 'Round-trip'
            anomalies = []
            
            # Skip if column doesn't exist in data
            if latency_col not in data.columns or data[latency_col].isna().all():
                return anomalies
            
            if 'spike_detection' in statistical_overlays and len(data) > 20:
                # Simple spike detection using z-score
                try:
                    from scipy.stats import zscore
                    z_scores = np.abs(zscore(data[latency_col]))
                    spike_threshold = 3.0
                    spikes = data[z_scores > spike_threshold]
                    
                    if len(spikes) > 0:
                        anomalies.append({
                            'type': 'spikes',
                            'data': spikes,
                            'description': f"{len(spikes)} latency spikes detected in {mechanism} ({type_label})"
                        })
                except ImportError:
                    pass  # Skip spike detection if scipy not available
            
            if 'anomaly_detection' in statistical_overlays and len(data) > 100:
                # Advanced anomaly detection using isolation forest
                if SKLEARN_AVAILABLE:
                    try:
                        clf = IsolationForest(contamination=0.1, random_state=42)
                        anomaly_labels = clf.fit_predict(data[[latency_col]].values)
                        anomaly_data = data[anomaly_labels == -1]
                        
                        if len(anomaly_data) > 0:
                            anomalies.append({
                                'type': 'anomalies',
                                'data': anomaly_data,
                                'description': f"{len(anomaly_data)} ML anomalies detected in {mechanism} ({type_label})"
                            })
                    except Exception:
                        # Fallback to simple statistical anomaly detection
                        q75 = data[latency_col].quantile(0.75)
                        q25 = data[latency_col].quantile(0.25)
                        iqr = q75 - q25
                        anomaly_threshold = q75 + 2.0 * iqr
                        anomaly_data = data[data[latency_col] > anomaly_threshold]
                        
                        if len(anomaly_data) > 0:
                            anomalies.append({
                                'type': 'statistical_anomalies',
                                'data': anomaly_data,
                                'description': f"{len(anomaly_data)} statistical anomalies in {mechanism} ({type_label})"
                            })

            
            return anomalies
        
        # Main chart generation logic based on layout type
        if chart_layout == 'faceted':
            # FACETED VIEW: Organize by message size
            message_sizes_available = sorted(streaming_df['message_size'].unique())
            
            for msg_size in message_sizes_available:
                size_data = streaming_df[streaming_df['message_size'] == msg_size]
                
                if len(size_data) > 0:
                    fig = go.Figure()
                    
                    for i, mechanism in enumerate(size_data['mechanism'].unique()):
                        symbol = symbols[i % len(symbols)]
                        color = mechanism_colors[i % len(mechanism_colors)]
                        
                        mechanism_df = size_data[size_data['mechanism'] == mechanism]
                        run_data = self._apply_sampling_strategy(mechanism_df, sampling_strategy, MAX_POINTS_PER_RUN)
                        run_data = run_data.reset_index(drop=True)
                        
                        # Add statistical overlays for each selected latency type
                        for latency_type in latency_types:
                            latency_col = 'one_way_latency_us' if latency_type == 'one_way' else 'round_trip_latency_us'
                            if latency_col in run_data.columns and not run_data[latency_col].isna().all():
                                overlay_color = self._get_mechanism_colors(mechanism, latency_type, list(streaming_df['mechanism'].unique()) if not streaming_df.empty else [])
                                add_statistical_overlays(fig, run_data, mechanism, overlay_color, latency_type)
                        

                        
                        # Generate traces for each selected latency type
                        for latency_type in latency_types:
                            latency_col = 'one_way_latency_us' if latency_type == 'one_way' else 'round_trip_latency_us'
                            type_label = 'One-way' if latency_type == 'one_way' else 'Round-trip'
                            trace_color = self._get_mechanism_colors(mechanism, latency_type, list(streaming_df['mechanism'].unique()) if not streaming_df.empty else [])
                            
                            # Skip if column doesn't exist in data
                            if latency_col not in run_data.columns or run_data[latency_col].isna().all():
                                continue
                                
                            if 'raw_dots' in display_options:
                                fig.add_trace(go.Scatter(
                                    x=run_data.index,
                                    y=run_data[latency_col],
                                    mode='markers',
                                    name=f'{mechanism} ({type_label})',
                                    marker=dict(color=trace_color, size=3, opacity=0.7, symbol=symbol),
                            showlegend=True
                        ))
                        
                            if 'moving_avg' in display_options and len(run_data) > moving_avg_window:
                                ma_col = f'{latency_type}_ma'
                                run_data[ma_col] = run_data[latency_col].rolling(window=moving_avg_window, min_periods=1).mean()
                                
                            fig.add_trace(go.Scatter(
                                x=run_data.index,
                                    y=run_data[ma_col],
                                mode='lines',
                                    name=f'{mechanism} ({type_label}) MA',
                                    line=dict(color=trace_color, width=2, dash='solid'),
                                opacity=0.9,
                                showlegend=True
                            ))
            
                    # Add performance zones if enabled
                    if 'perf_zones' in view_options and len(size_data) > 0:
                        # Add performance zones for each selected latency type
                        for latency_type in latency_types:
                            latency_col = 'one_way_latency_us' if latency_type == 'one_way' else 'round_trip_latency_us'
                            type_label = 'One-way' if latency_type == 'one_way' else 'Round-trip'
                            
                            if latency_col in size_data.columns and not size_data[latency_col].isna().all():
                                overall_p50 = size_data[latency_col].quantile(0.5)
                                overall_p95 = size_data[latency_col].quantile(0.95)
                                
                                # Use distinct colors for performance zones of different latency types
                                zone_color_p50 = '#10b981' if latency_type == 'one_way' else '#059669'  # Green family
                                zone_color_p95 = '#f59e0b' if latency_type == 'one_way' else '#d97706'  # Orange family
                                
                                fig.add_hline(y=overall_p50, line_dash="dash", line_color=zone_color_p50, 
                                             annotation_text=f"Good P50 ({type_label})", annotation_position="bottom right")
                                fig.add_hline(y=overall_p95, line_dash="dash", line_color=zone_color_p95, 
                                             annotation_text=f"Warning P95 ({type_label})", annotation_position="bottom right")
                    
                    # Update chart layout for this message size (moved inside chart creation block)
                    fig.update_layout(
                        title=f"Message Size: {msg_size} bytes",
                        xaxis_title="Sample Index", 
                        yaxis_title="Latency (μs)",
                        yaxis_type=y_axis_scale,
                        yaxis_autorange=True if y_axis_range == 'auto' else False,
                        showlegend=True,
                        font=dict(size=12),
                        height=500,
                        template='plotly_white',
                        legend=dict(
                            orientation="h",
                            yanchor="top",
                            y=-0.15,
                            xanchor="center",
                            x=0.5,
                            bgcolor="rgba(255,255,255,0.9)",
                            bordercolor="rgba(0,0,0,0.2)",
                            borderwidth=1
                        ),
                        margin=dict(b=100)
                    )
            
                    charts.append(html.Div([
                        dcc.Graph(figure=self.apply_dark_theme(fig), id=f'faceted-chart-{msg_size}')
                    ], className="chart-container", style={'width': '100%', 'margin-bottom': '20px'}))
        
        else:
            # SEPARATE VIEW: Original behavior with enhancements
            for i, mechanism in enumerate(streaming_df['mechanism'].unique()):
                symbol = symbols[i % len(symbols)]
                color = mechanism_colors[i % len(mechanism_colors)]
                fig = go.Figure()
                
                # Filter data for this mechanism
                mechanism_df = streaming_df[streaming_df['mechanism'] == mechanism]
                
                if len(mechanism_df) > 0:
                    # Get unique message sizes for this mechanism to create separate traces
                    mechanism_message_sizes = sorted(mechanism_df['message_size'].unique())
                    
                    # Color palette for message sizes (distinct colors)
                    size_colors = ['#1f77b4', '#ff7f0e', '#2ca02c', '#d62728', '#9467bd', '#8c564b', '#e377c2', '#7f7f7f']
                    
                    anomalies = []
                    
                    # Create traces for each message size within this mechanism
                    for size_idx, msg_size in enumerate(mechanism_message_sizes):
                        # Filter data for this specific message size
                        size_df = mechanism_df[mechanism_df['message_size'] == msg_size]
                        
                        if len(size_df) == 0:
                            continue
                            
                        run_data = self._apply_sampling_strategy(size_df, sampling_strategy, MAX_POINTS_PER_RUN // len(mechanism_message_sizes))
                        run_data = run_data.reset_index(drop=True)
                        
                        # Use different colors for each message size
                        size_color = size_colors[size_idx % len(size_colors)]
                        
                        # Detect anomalies for each selected latency type for this message size
                        for latency_type in latency_types:
                            latency_col = 'one_way_latency_us' if latency_type == 'one_way' else 'round_trip_latency_us'
                            if latency_col in run_data.columns and not run_data[latency_col].isna().all():
                                type_anomalies = self._detect_anomalies(run_data, mechanism, latency_type, statistical_overlays)
                                anomalies.extend(type_anomalies)
                        
                        # Generate traces for each selected latency type and message size combination
                        for latency_type in latency_types:
                            latency_col = 'one_way_latency_us' if latency_type == 'one_way' else 'round_trip_latency_us'
                            type_label = 'One-way' if latency_type == 'one_way' else 'Round-trip'
                            
                            # Skip if column doesn't exist in data
                            if latency_col not in run_data.columns or run_data[latency_col].isna().all():
                                continue
                            
                            # Create trace name with both message size and latency type
                            trace_name = f'{msg_size}B - {type_label}'
                            
                            if 'raw_dots' in display_options:
                                fig.add_trace(go.Scatter(
                                    x=run_data.index,
                                    y=run_data[latency_col],
                                    mode='markers',
                                    name=trace_name,
                                    marker=dict(color=size_color, size=3, opacity=0.7, symbol=symbol),
                                    showlegend=True
                                ))
                                
                            if 'moving_avg' in display_options and len(run_data) > moving_avg_window:
                                ma_col = f'{latency_type}_ma'
                                run_data[ma_col] = run_data[latency_col].rolling(window=moving_avg_window, min_periods=1).mean()
                                        
                                fig.add_trace(go.Scatter(
                                    x=run_data.index,
                                    y=run_data[ma_col],
                                    mode='lines',
                                    name=f'{trace_name} MA',
                                    line=dict(color=size_color, width=2, dash='solid'),
                                    opacity=0.9,
                                    showlegend=True
                                ))
                    
                    # Add statistical overlays per message size for better granularity
                    for size_idx, msg_size in enumerate(mechanism_message_sizes):
                        size_df = mechanism_df[mechanism_df['message_size'] == msg_size]
                        if len(size_df) == 0:
                            continue
                            
                        run_data_overlay = self._apply_sampling_strategy(size_df, sampling_strategy, MAX_POINTS_PER_RUN // len(mechanism_message_sizes))
                        run_data_overlay = run_data_overlay.reset_index(drop=True)
                        
                        # Use the same color as the corresponding trace
                        size_color = size_colors[size_idx % len(size_colors)]
                        
                        for latency_type in latency_types:
                            latency_col = 'one_way_latency_us' if latency_type == 'one_way' else 'round_trip_latency_us'
                            if latency_col in run_data_overlay.columns and not run_data_overlay[latency_col].isna().all():
                                # Add statistical overlays with message size context
                                add_statistical_overlays(fig, run_data_overlay, f"{mechanism} {msg_size}B", size_color, latency_type)
                    
                    anomaly_info = ""
                    if anomalies:
                        anomaly_info = f" ({len(anomalies)} anomalies)"
                
                # Add performance zones if enabled - show overall mechanism zones for reference
                if 'perf_zones' in view_options and len(mechanism_df) > 0:
                    # Add overall mechanism performance zones for each selected latency type
                    for latency_type in latency_types:
                        latency_col = 'one_way_latency_us' if latency_type == 'one_way' else 'round_trip_latency_us'
                        type_label = 'One-way' if latency_type == 'one_way' else 'Round-trip'
                        
                        if latency_col in mechanism_df.columns and not mechanism_df[latency_col].isna().all():
                            overall_p50 = mechanism_df[latency_col].quantile(0.5)
                            overall_p95 = mechanism_df[latency_col].quantile(0.95)
                            
                            # Use distinct colors for performance zones of different latency types
                            zone_color_p50 = '#10b981' if latency_type == 'one_way' else '#059669'  # Green family
                            zone_color_p95 = '#f59e0b' if latency_type == 'one_way' else '#d97706'  # Orange family
                            
                            fig.add_hline(y=overall_p50, line_dash="dash", line_color=zone_color_p50, 
                                         annotation_text=f"Overall P50 ({type_label})", annotation_position="bottom right")
                            fig.add_hline(y=overall_p95, line_dash="dash", line_color=zone_color_p95, 
                                         annotation_text=f"Overall P95 ({type_label})", annotation_position="bottom right")
                
                # Update chart layout for this mechanism (moved outside performance zones block)
                fig.update_layout(
                    title=f"{mechanism}{anomaly_info}",
                    xaxis_title="Sample Index",
                    yaxis_title="Latency (μs)",
                    yaxis_type=y_axis_scale,
                    yaxis_autorange=True if y_axis_range == 'auto' else False,
                    showlegend=True,
                    font=dict(size=12),
                    height=550,
                    template='plotly_white',
                    legend=dict(
                        orientation="h",
                        yanchor="top",
                        y=-0.15,
                        xanchor="center",
                        x=0.5,
                        bgcolor="rgba(255,255,255,0.9)",
                        bordercolor="rgba(0,0,0,0.2)",
                        borderwidth=1
                    ),
                    margin=dict(b=100)
                )
                
                charts.append(html.Div([
                    dcc.Graph(figure=self.apply_dark_theme(fig), id=f'separate-chart-{mechanism}')
                ], className="chart-container"))
        
        # Combine all elements
        return html.Div([
            performance_note,
            *charts,
            stats_panel
        ])
    
    def run(self, host='127.0.0.1', port=8050, debug=True):
        """Run the dashboard server."""
        logger.info(f"Starting dashboard at http://{host}:{port}")
        self.app.run(host=host, port=port, debug=debug)

def main():
    """Main entry point."""
    parser = argparse.ArgumentParser(description="Rusty-Comms Benchmark Results Dashboard")
    parser.add_argument("--dir", required=True, help="Directory containing benchmark result files")
    parser.add_argument("--host", default="127.0.0.1", help="Host to bind to (default: 127.0.0.1)")
    parser.add_argument("--port", type=int, default=8050, help="Port to bind to (default: 8050)")
    parser.add_argument("--debug", action="store_true", default=False, help="Enable debug mode (can cause reloading issues)")
    
    args = parser.parse_args()
    
    # Validate directory
    results_dir = Path(args.dir)
    if not results_dir.exists():
        logger.error(f"Results directory does not exist: {results_dir}")
        return 1
    
    # Process data
    processor = BenchmarkDataProcessor(results_dir)
    data_store = processor.process_all_data()
    
    if data_store['summary_data'].empty and data_store['streaming_data'].empty:
        logger.error("No valid benchmark data found in the specified directory")
        return 1
    
    # Setup and run dashboard
    dashboard = DashboardApp(data_store, initial_dir=args.dir)
    dashboard.run(host=args.host, port=args.port, debug=args.debug)
    
    return 0

# Modern CSS styling
# CSS styles are now loaded from external file: dashboard_styles.css

if __name__ == "__main__":
    exit(main())
