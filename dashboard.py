#!/usr/bin/env python3
"""
Interactive Dashboard for Rusty-Comms Benchmark Results

A comprehensive Plotly Dash application for visualizing IPC benchmark results
from both JSON and CSV output formats with interactive filtering and analysis.
"""

import argparse
import json
import logging
import pandas as pd
import plotly.express as px
import plotly.graph_objects as go
from plotly.subplots import make_subplots
import dash
from dash import dcc, html, Input, Output, State, callback, dash_table
import numpy as np
from pathlib import Path
from typing import Dict, List, Tuple, Optional, Union
import hashlib
from datetime import datetime

# Configure logging
logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)

class BenchmarkDataProcessor:
    """Handles discovery, loading, and processing of benchmark result files."""
    
    def __init__(self, results_dir: Path):
        self.results_dir = Path(results_dir)
        self.data_store = {
            'summary_data': pd.DataFrame(),
            'streaming_data': pd.DataFrame(),
            'runs_metadata': {}
        }
    
    def discover_files(self) -> Dict[str, List[Path]]:
        """Discover and categorize all benchmark result files."""
        files = {
            'summary_json': [],
            'streaming_json': [],
            'streaming_csv': [],
            'unknown': []
        }
        
        for file_path in self.results_dir.rglob('*.json'):
            try:
                file_type = self._detect_json_type(file_path)
                if file_type in ['summary_json', 'streaming_json']:
                    files[file_type].append(file_path)
                else:
                    files['unknown'].append(file_path)
            except Exception as e:
                logger.warning(f"Could not process {file_path}: {e}")
                files['unknown'].append(file_path)
        
        for file_path in self.results_dir.rglob('*.csv'):
            try:
                if self._detect_csv_type(file_path):
                    files['streaming_csv'].append(file_path)
                else:
                    files['unknown'].append(file_path)
            except Exception as e:
                logger.warning(f"Could not process {file_path}: {e}")
                files['unknown'].append(file_path)
        
        return files
    
    def _detect_json_type(self, file_path: Path) -> str:
        """Detect JSON file type based on structure."""
        try:
            with open(file_path, 'r') as f:
                data = json.load(f)
            
            # Summary JSON: has "results" with elements containing "summary"
            if isinstance(data, dict) and "results" in data:
                if isinstance(data["results"], list) and len(data["results"]) > 0:
                    if "summary" in data["results"][0]:
                        return "summary_json"
            
            # Streaming JSON: has "headings" and "data" keys
            if isinstance(data, dict) and "headings" in data and "data" in data:
                return "streaming_json"
                
            return "unknown"
        except (json.JSONDecodeError, KeyError):
            return "unknown"
    
    def _detect_csv_type(self, file_path: Path) -> bool:
        """Detect if CSV is streaming data based on columns."""
        try:
            df_sample = pd.read_csv(file_path, nrows=1)
            streaming_columns = {'one_way_latency_ns', 'round_trip_latency_ns', 'timestamp_ns', 'message_id'}
            return bool(streaming_columns.intersection(set(df_sample.columns)))
        except Exception:
            return False
    

    
    def load_summary_data(self, files: List[Path]) -> pd.DataFrame:
        """Load and process summary JSON files."""
        summary_records = []
        
        for file_path in files:
            try:
                with open(file_path, 'r') as f:
                    data = json.load(f)
                
                for result in data.get('results', []):
                    mechanism = result.get('mechanism', 'unknown')
                    # message_size is inside test_config
                    test_config = result.get('test_config', {})
                    message_size = test_config.get('message_size')
                    if message_size is None or message_size <= 0:
                        logger.warning(f"Skipping result with invalid message_size: {message_size}")
                        continue
                    
                    summary = result.get('summary', {})
                    
                    # Extract latency percentiles (convert ns to μs)
                    latency_data = summary.get('latency', {}).get('percentiles', [])
                    percentiles = {p['percentile']: p['value_ns'] / 1000 for p in latency_data}
                    
                    # Extract throughput data
                    one_way_throughput = result.get('one_way_results', {}).get('throughput', {})
                    round_trip_throughput = result.get('round_trip_results', {}).get('throughput', {})
                    
                    record = {
                        'mechanism': mechanism,
                        'message_size': message_size,
                        'file_path': str(file_path),
                        'p50_latency_us': percentiles.get(50.0, np.nan),
                        'p95_latency_us': percentiles.get(95.0, np.nan),
                        'p99_latency_us': percentiles.get(99.0, np.nan),
                        'mean_latency_us': summary.get('latency', {}).get('mean_ns', 0) / 1000,
                        'one_way_msgs_per_sec': one_way_throughput.get('messages_per_second', np.nan),
                        'one_way_bytes_per_sec': one_way_throughput.get('bytes_per_second', np.nan),
                        'round_trip_msgs_per_sec': round_trip_throughput.get('messages_per_second', np.nan),
                        'round_trip_bytes_per_sec': round_trip_throughput.get('bytes_per_second', np.nan),
                    }
                    summary_records.append(record)
                    
            except Exception as e:
                logger.error(f"Error processing summary file {file_path}: {e}")
        
        return pd.DataFrame(summary_records)
    
    def load_streaming_data(self, json_files: List[Path], csv_files: List[Path]) -> pd.DataFrame:
        """Load and process streaming data, preferring JSON over CSV for same runs."""
        streaming_records = []
        
        # Process JSON files first (preferred)
        for file_path in json_files:
            try:
                with open(file_path, 'r') as f:
                    data = json.load(f)
                
                headings = data.get('headings', [])
                records = data.get('data', [])
                
                if not headings or not records:
                    continue
                
                df = pd.DataFrame(records, columns=headings)
                
                # Detect mechanism and message size from first record
                if len(df) > 0:
                    mechanism = df['mechanism'].iloc[0] if 'mechanism' in df.columns else 'unknown'
                    message_size = df['message_size'].iloc[0] if 'message_size' in df.columns else None
                    
                    if message_size is None or message_size <= 0:
                        logger.warning(f"Skipping streaming data with invalid message_size: {message_size} in {file_path}")
                        continue
                        
                    # Add file path for reference
                    df['file_path'] = str(file_path)
                    df['file_type'] = 'streaming_json'
                    
                    # Convert latencies from ns to μs
                    if 'one_way_latency_ns' in df.columns:
                        df['one_way_latency_us'] = df['one_way_latency_ns'] / 1000
                    if 'round_trip_latency_ns' in df.columns:
                        df['round_trip_latency_us'] = df['round_trip_latency_ns'] / 1000
                    
                    streaming_records.append(df)
                        
            except Exception as e:
                logger.error(f"Error processing streaming JSON {file_path}: {e}")
        
        # Process CSV files (only if not already processed as JSON)
        for file_path in csv_files:
            try:
                df = pd.read_csv(file_path)
                
                if len(df) > 0:
                    mechanism = df['mechanism'].iloc[0] if 'mechanism' in df.columns else 'unknown'
                    message_size = df['message_size'].iloc[0] if 'message_size' in df.columns else 0
                    
                    if message_size > 0:  # Only process valid message sizes
                        df['file_path'] = str(file_path)
                        df['file_type'] = 'streaming_csv'
                        
                        # Convert latencies from ns to μs
                        if 'one_way_latency_ns' in df.columns:
                            df['one_way_latency_us'] = df['one_way_latency_ns'] / 1000
                        if 'round_trip_latency_ns' in df.columns:
                            df['round_trip_latency_us'] = df['round_trip_latency_ns'] / 1000
                        
                        streaming_records.append(df)
                        
            except Exception as e:
                logger.error(f"Error processing streaming CSV {file_path}: {e}")
        
        if streaming_records:
            return pd.concat(streaming_records, ignore_index=True)
        else:
            return pd.DataFrame()
    
    def process_all_data(self) -> Dict:
        """Discover and process all benchmark data files."""
        logger.info(f"Discovering files in {self.results_dir}")
        files = self.discover_files()
        
        logger.info(f"Found {len(files['summary_json'])} summary JSON, "
                   f"{len(files['streaming_json'])} streaming JSON, "
                   f"{len(files['streaming_csv'])} streaming CSV files")
        
        # Load summary data
        if files['summary_json']:
            self.data_store['summary_data'] = self.load_summary_data(files['summary_json'])
            logger.info(f"Loaded {len(self.data_store['summary_data'])} summary records")
        
        # Load streaming data
        if files['streaming_json'] or files['streaming_csv']:
            self.data_store['streaming_data'] = self.load_streaming_data(
                files['streaming_json'], files['streaming_csv']
            )
            logger.info(f"Loaded {len(self.data_store['streaming_data'])} streaming records")
        
        return self.data_store

class DashboardApp:
    """Main Dash application for interactive benchmark visualization."""
    
    def __init__(self, data_store: Dict, initial_dir: str = "."):
        self.data_store = data_store
        self.initial_dir = initial_dir
        self.app = dash.Dash(__name__)
        # Inject modern CSS
        self.app.index_string = '''
        <!DOCTYPE html>
        <html>
            <head>
                {%metas%}
                <title>{%title%}</title>
                {%favicon%}
                {%css%}
                <style>
                ''' + app_css + '''
                </style>
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
        self.setup_layout()
        self.setup_callbacks()
    
    def reload_data(self, directory_path: str) -> tuple:
        """Reload data from a new directory and update the data store."""
        try:
            from pathlib import Path
            
            # Validate directory path
            dir_path = Path(directory_path)
            if not dir_path.exists():
                return False, f"❌ Error: Directory '{directory_path}' does not exist."
            
            if not dir_path.is_dir():
                return False, f"❌ Error: '{directory_path}' is not a directory."
            
            # Load data from new directory
            logger.info(f"Reloading data from directory: {directory_path}")
            data_loader = BenchmarkDataProcessor(dir_path)
            new_data_store = data_loader.process_all_data()
            
            # Check if any data was found
            summary_count = len(new_data_store['summary_data'])
            streaming_count = len(new_data_store['streaming_data'])
            
            if summary_count == 0 and streaming_count == 0:
                return False, f"⚠️ Warning: No benchmark data files found in '{directory_path}'."
            
            # Update the data store
            self.data_store.update(new_data_store)
            
            # Success message
            success_msg = f"✅ Success: Loaded {summary_count} summary records and {streaming_count:,} streaming records from '{directory_path}'."
            logger.info(success_msg)
            
            return True, success_msg
            
        except Exception as e:
            error_msg = f"❌ Error loading data: {str(e)}"
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
                    
                    # Data Directory Controls
                    html.Div([
                        html.H3("Data Directory"),
                        html.P("Select a directory containing benchmark data files.", 
                               style={'color': '#6b7280', 'font-size': '0.9rem', 'margin-bottom': '10px', 'font-style': 'italic'}),
                        
                        html.Div([
                            dcc.Dropdown(
                                id='data-directory-dropdown',
                                options=[
                                    {'label': '📁 Current Directory (.)', 'value': '.'},
                                    {'label': '📁 Parent Directory (..)', 'value': '..'},
                                    {'label': '📁 Home Directory (~)', 'value': '~'},
                                    {'label': '📁 Desktop (~/Desktop)', 'value': '~/Desktop'},
                                    {'label': '📁 Downloads (~/Downloads)', 'value': '~/Downloads'},
                                    {'label': '📁 Documents (~/Documents)', 'value': '~/Documents'},
                                    {'label': '🔧 Custom Path...', 'value': 'custom'}
                                ],
                                value='.',
                                clearable=False,
                                style={'margin-bottom': '10px'}
                            ),
                            
                            # Custom path input (hidden by default)
                            html.Div([
                                dcc.Input(
                                    id='custom-directory-input',
                                    type='text',
                                    placeholder='Enter custom directory path...',
                                    value='',
                                    style={
                                        'width': '100%', 
                                        'padding': '8px', 
                                        'border': '1px solid #d1d5db',
                                        'border-radius': '4px',
                                        'font-size': '0.9rem'
                                    }
                                )
                            ], id='custom-path-container', style={'display': 'none', 'margin-bottom': '10px'}),
                            
                            html.Button(
                                '🔄 Reload Data', 
                                id='reload-data-button',
                                n_clicks=0,
                                style={
                                    'width': '100%',
                                    'padding': '10px 16px',
                                    'background-color': '#3b82f6',
                                    'color': 'white',
                                    'border': 'none',
                                    'border-radius': '6px',
                                    'cursor': 'pointer',
                                    'font-size': '0.95rem',
                                    'font-weight': '600'
                                }
                            ),
                        ], style={'margin-bottom': '20px'}),
                        
                        # Status message for reload feedback
                        html.Div(id='reload-status', style={'margin-bottom': '15px'})
                    ], className="filter-section"),
                    
                    html.Div([
                        html.H3("Filters"),
                        html.P("Select items to filter data. Leave empty to show all.", 
                               style={'color': '#6b7280', 'font-size': '0.9rem', 'margin-bottom': '15px', 'font-style': 'italic'}),
                        
                        html.Label("Mechanisms:"),
                        dcc.Checklist(
                            id='mechanism-filter',
                            options=[{'label': m, 'value': m} for m in filter_options['mechanisms']],
                            value=[],  # Start with no filters selected (show all data)
                            className="filter-checklist"
                        ),
                    ], className="filter-section"),
                    
                    html.Div([
                        html.Label("Message Sizes:"),
                        dcc.Checklist(
                            id='message-size-filter',
                            options=[{'label': f"{s}B", 'value': s} for s in filter_options['message_sizes']],
                            value=[],  # Start with no filters selected (show all data)
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
                            'marginBottom': '25px'
                        },
                        children=[
                            dcc.Tab(
                                label="Summary", 
                                value="summary-tab",
                                style={'padding': '12px 24px', 'fontWeight': '500'},
                                selected_style={'padding': '12px 24px', 'fontWeight': '600', 'color': '#667eea'}
                            ),
                            dcc.Tab(
                                label="Time Series", 
                                value="timeseries-tab",
                                style={'padding': '12px 24px', 'fontWeight': '500'},
                                selected_style={'padding': '12px 24px', 'fontWeight': '600', 'color': '#667eea'}
                            ),

                        ]
                    ),
                    html.Div(id="tab-content", className="tab-content")
                ], className="main-content")
                
            ], className="dashboard-container")
        ], style={'background': 'linear-gradient(135deg, #667eea 0%, #764ba2 100%)', 'minHeight': '100vh'})
    
    def setup_callbacks(self):
        """Setup all dashboard callbacks."""
        
        # Setup initial components for threshold callbacks
        available_mechanisms = self.get_filter_options()['mechanisms']
        
        @self.app.callback(
            Output("tab-content", "children"),
            [Input("main-tabs", "value"),
             Input("mechanism-filter", "value"),
             Input("message-size-filter", "value")],
            prevent_initial_call=False
        )
        def render_tab_content(active_tab, mechanisms, message_sizes):
            if active_tab == "summary-tab":
                logger.info(f"Summary tab called with filters: mechanisms={mechanisms}, message_sizes={message_sizes}")
                return self.render_summary_tab(mechanisms, message_sizes)
            elif active_tab == "timeseries-tab":
                return self.render_timeseries_tab(mechanisms, message_sizes)
            return html.Div("Select a tab")
        
        # Dynamic threshold controls callback
        @self.app.callback(
            Output('threshold-controls-container', 'children'),
            [Input("mechanism-filter", "value")],
            prevent_initial_call=False
        )
        def update_threshold_controls(mechanisms):
            if not mechanisms or len(mechanisms) == 0:
                return [
                    html.Label("Outlier Thresholds (μs):", style={'margin-bottom': '15px', 'display': 'block', 'font-weight': 'bold'}),
                    html.P("Select mechanisms in the filter to set thresholds.", 
                           style={'color': '#6b7280', 'font-size': '0.9rem', 'margin-bottom': '15px', 'font-style': 'italic'}),
                ]
            
            threshold_controls = [
                html.Label("Outlier Thresholds (μs):", style={'margin-bottom': '15px', 'display': 'block', 'font-weight': 'bold'}),
                html.P("Set thresholds to highlight outliers in red (leave empty for no outlier detection).", 
                       style={'color': '#6b7280', 'font-size': '0.9rem', 'margin-bottom': '15px', 'font-style': 'italic'}),
            ]
            
            # Create threshold inputs for each selected mechanism using pattern-matching IDs
            threshold_inputs = []
            for mechanism in mechanisms:
                threshold_inputs.append(html.Div([
                    html.Label(f"{mechanism}:", style={'margin-bottom': '5px', 'display': 'block'}),
                    dcc.Input(
                        id={'type': 'threshold-input', 'mechanism': mechanism},
                        type='number',
                        placeholder='Enter threshold',
                        value=None,
                        min=0,
                        style={'width': '100%', 'margin-bottom': '10px'}
                    )
                ], style={'width': f'{95//len(mechanisms)}%', 'display': 'inline-block', 'vertical-align': 'top', 'margin-right': '2%'}))
            
            threshold_controls.append(html.Div(threshold_inputs))
            return threshold_controls

        # Time series charts update callback (with pattern-matching for thresholds)
        @self.app.callback(
            Output('ts-charts-container', 'children'),
            [Input('ts-moving-avg-slider', 'value'),
             Input('ts-display-options', 'value'),
             Input('ts-dot-size-slider', 'value'),
             Input('ts-line-width-slider', 'value'),
             Input("mechanism-filter", "value"),
             Input("message-size-filter", "value"), 
             Input({'type': 'threshold-input', 'mechanism': dash.dependencies.ALL}, 'value')],
            prevent_initial_call=False
        )
        def update_timeseries_charts(moving_avg_window, display_options, dot_size, line_width, mechanisms, message_sizes, threshold_values):
            from dash import ctx
            
            # Handle None values for size controls
            dot_size = dot_size if dot_size is not None else 3
            line_width = line_width if line_width is not None else 2
            
            # Extract threshold values and map them to mechanisms
            thresholds = {}
            if mechanisms and len(mechanisms) > 0:
                try:
                    # Get the IDs of the threshold inputs that triggered this callback
                    # Pattern-matching inputs provide their IDs in ctx.inputs_list
                    threshold_inputs_list = ctx.inputs_list[-1]  # Last input is the pattern-matching one
                    
                    # Create mapping from mechanism to threshold value
                    for threshold_input in threshold_inputs_list:
                        if threshold_input['id']['type'] == 'threshold-input':
                            mechanism_name = threshold_input['id']['mechanism']
                            threshold_value = threshold_input['value']
                            thresholds[mechanism_name] = threshold_value
                    
                    # Ensure all selected mechanisms have threshold entries (even if None)
                    for mech in mechanisms:
                        if mech not in thresholds:
                            thresholds[mech] = None
                            
                except Exception as e:
                    # If there's any issue extracting from context, set all thresholds to None
                    for mech in mechanisms if mechanisms else []:
                        thresholds[mech] = None
            
            return self.render_timeseries_charts(mechanisms, message_sizes, moving_avg_window, display_options, thresholds, dot_size, line_width)
        
        # Directory dropdown callback to show/hide custom input
        @self.app.callback(
            Output('custom-path-container', 'style'),
            [Input('data-directory-dropdown', 'value')],
            prevent_initial_call=False
        )
        def toggle_custom_input(dropdown_value):
            if dropdown_value == 'custom':
                return {'display': 'block', 'margin-bottom': '10px'}
            else:
                return {'display': 'none', 'margin-bottom': '10px'}
        
        # Data reload callback
        @self.app.callback(
            [Output('reload-status', 'children'),
             Output('mechanism-filter', 'options'),
             Output('mechanism-filter', 'value'),
             Output('message-size-filter', 'options'),
             Output('message-size-filter', 'value')],
            [Input('reload-data-button', 'n_clicks')],
            [State('data-directory-dropdown', 'value'),
             State('custom-directory-input', 'value')],
            prevent_initial_call=True
        )
        def handle_data_reload(n_clicks, dropdown_value, custom_path):
            if n_clicks == 0:
                # No callback needed for initial state
                raise dash.exceptions.PreventUpdate
            
            # Determine the directory path to use
            if dropdown_value == 'custom':
                directory_path = custom_path if custom_path else '.'
            else:
                directory_path = dropdown_value
            
            # Expand ~ for home directory
            if directory_path.startswith('~'):
                import os
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
                
                # Return updated filter options and clear selections
                return (
                    status_div,
                    [{'label': m, 'value': m} for m in new_filter_options['mechanisms']],
                    [],  # Clear mechanism filter selection
                    [{'label': str(s), 'value': s} for s in new_filter_options['message_sizes']],
                    []   # Clear message size filter selection
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
                
                # Keep existing filter options on error
                current_filter_options = self.get_filter_options()
                return (
                    status_div,
                    [{'label': m, 'value': m} for m in current_filter_options['mechanisms']],
                    [],  # Clear mechanism filter selection
                    [{'label': str(s), 'value': s} for s in current_filter_options['message_sizes']],
                    []   # Clear message size filter selection
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
    
    def generate_performance_insights(self, percentile_df: pd.DataFrame, throughput_df: pd.DataFrame = None, max_latency_df: pd.DataFrame = None) -> Dict:
        """Generate smart insights and recommendations from the data."""
        insights = {
            'best_mechanism': None,
            'best_latency': None,
            'best_max_latency': None,
            'best_max_mechanism': None,
            'best_throughput': None,
            'recommendations': [],
            'summary_stats': {}
        }
        
        if percentile_df.empty:
            return insights
            
        # Find best mechanism for latency (lowest P50)
        one_way_p50 = percentile_df[
            (percentile_df['latency_type'] == 'One-way') & 
            (percentile_df['percentile'] == 'P50 (Median)')
        ]
        
        if not one_way_p50.empty:
            best_latency_row = one_way_p50.loc[one_way_p50['latency_us'].idxmin()]
            insights['best_mechanism'] = best_latency_row['mechanism']
            insights['best_latency'] = best_latency_row['latency_us']
            insights['summary_stats']['best_p50'] = best_latency_row['latency_us']
            
            # Calculate performance advantages
            worst_latency = one_way_p50['latency_us'].max()
            if worst_latency > insights['best_latency']:
                improvement = ((worst_latency - insights['best_latency']) / worst_latency) * 100
                insights['recommendations'].append(
                    f"{insights['best_mechanism']} is {improvement:.1f}% faster than the slowest mechanism"
                )
        
        # Max latency analysis
        if max_latency_df is not None and not max_latency_df.empty:
            one_way_max = max_latency_df[max_latency_df['latency_type'] == 'One-way']
            if not one_way_max.empty:
                best_max_row = one_way_max.loc[one_way_max['max_latency_us'].idxmin()]
                insights['best_max_latency'] = best_max_row['max_latency_us']
                insights['best_max_mechanism'] = best_max_row['mechanism']
                insights['summary_stats']['best_max_latency'] = best_max_row['max_latency_us']
                
                # Calculate max latency advantages
                worst_max_latency = one_way_max['max_latency_us'].max()
                if worst_max_latency > insights['best_max_latency']:
                    max_improvement = ((worst_max_latency - insights['best_max_latency']) / worst_max_latency) * 100
                    insights['recommendations'].append(
                        f"{insights['best_max_mechanism']} has {max_improvement:.1f}% better worst-case latency performance"
                    )
                
                # Compare with P50 best mechanism for consistency
                if insights['best_mechanism'] and insights['best_max_mechanism'] != insights['best_mechanism']:
                    insights['recommendations'].append(
                        f"Different mechanisms excel at typical ({insights['best_mechanism']}) vs worst-case ({insights['best_max_mechanism']}) performance"
                    )
        
        # Throughput analysis
        if throughput_df is not None and not throughput_df.empty:
            one_way_throughput = throughput_df[throughput_df['type'] == 'One-way']
            if not one_way_throughput.empty:
                best_throughput_row = one_way_throughput.loc[one_way_throughput['msgs_per_sec'].idxmax()]
                insights['best_throughput'] = {
                    'mechanism': best_throughput_row['mechanism'],
                    'msgs_per_sec': best_throughput_row['msgs_per_sec'],
                    'message_size': best_throughput_row['message_size']
                }
        
        # Generate message size recommendations
        if len(one_way_p50) > 1:
            for message_size in one_way_p50['message_size'].unique():
                size_data = one_way_p50[one_way_p50['message_size'] == message_size]
                if len(size_data) > 1:
                    best_for_size = size_data.loc[size_data['latency_us'].idxmin()]
                    insights['recommendations'].append(
                        f"For {message_size}B messages: {best_for_size['mechanism']} performs best ({best_for_size['latency_us']:.1f}μs P50)"
                    )
        
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
                html.H4("🏆 Best Overall", style={'margin': '0', 'color': '#059669', 'font-size': '0.9rem'}),
                html.H2(insights.get('best_mechanism', 'N/A'), style={'margin': '5px 0', 'color': '#065f46'}),
                html.P(f"P50: {insights.get('best_latency', 0):.1f}μs", style={'margin': '0', 'color': '#6b7280', 'font-size': '0.9rem'})
            ], style={
                'background': 'linear-gradient(135deg, #d1fae5 0%, #a7f3d0 100%)',
                'padding': '20px', 'border-radius': '12px', 'text-align': 'center',
                'box-shadow': '0 4px 6px rgba(0, 0, 0, 0.1)', 'border': '1px solid #10b981'
            })
        ], style={'flex': '1', 'min-width': '0'})
        
        # Performance Stats Card
        one_way_p50 = percentile_df[
            (percentile_df['latency_type'] == 'One-way') & 
            (percentile_df['percentile'] == 'P50 (Median)')
        ]
        
        min_latency = one_way_p50['latency_us'].min() if not one_way_p50.empty else 0
        max_latency = one_way_p50['latency_us'].max() if not one_way_p50.empty else 0
        avg_latency = one_way_p50['latency_us'].mean() if not one_way_p50.empty else 0
        
        performance_card = html.Div([
            html.Div([
                html.H4("📊 Performance Range", style={'margin': '0', 'color': '#1e40af', 'font-size': '0.9rem'}),
                html.H2(f"{min_latency:.1f} - {max_latency:.1f}μs", style={'margin': '5px 0', 'color': '#1e3a8a'}),
                html.P(f"Average: {avg_latency:.1f}μs", style={'margin': '0', 'color': '#6b7280', 'font-size': '0.9rem'})
            ], style={
                'background': 'linear-gradient(135deg, #dbeafe 0%, #bfdbfe 100%)',
                'padding': '20px', 'border-radius': '12px', 'text-align': 'center',
                'box-shadow': '0 4px 6px rgba(0, 0, 0, 0.1)', 'border': '1px solid #3b82f6'
            })
        ], style={'flex': '1', 'min-width': '0'})
        
        # Throughput Card
        throughput_info = insights.get('best_throughput', {})
        throughput_card = html.Div([
            html.Div([
                html.H4("🚀 Peak Throughput", style={'margin': '0', 'color': '#7c2d12', 'font-size': '0.9rem'}),
                html.H2(f"{throughput_info.get('msgs_per_sec', 0):,.0f}", style={'margin': '5px 0', 'color': '#92400e'}),
                html.P(f"msgs/sec ({throughput_info.get('mechanism', 'N/A')})", style={'margin': '0', 'color': '#6b7280', 'font-size': '0.9rem'})
            ], style={
                'background': 'linear-gradient(135deg, #fef3c7 0%, #fde68a 100%)',
                'padding': '20px', 'border-radius': '12px', 'text-align': 'center',
                'box-shadow': '0 4px 6px rgba(0, 0, 0, 0.1)', 'border': '1px solid #f59e0b'
            })
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
                        html.H4("⚡ Best Max Latency", style={'margin': '0', 'color': '#dc2626', 'font-size': '0.9rem'}),
                        html.H2(f"{best_max_latency:.1f}μs", style={'margin': '5px 0', 'color': '#991b1b'}),
                        html.P(f"{best_max_mechanism}", style={'margin': '0', 'color': '#6b7280', 'font-size': '0.9rem'})
                    ], style={
                        'background': 'linear-gradient(135deg, #fee2e2 0%, #fecaca 100%)',
                        'padding': '20px', 'border-radius': '12px', 'text-align': 'center',
                        'box-shadow': '0 4px 6px rgba(0, 0, 0, 0.1)', 'border': '1px solid #dc2626'
                    })
                ], style={'flex': '1', 'min-width': '0'})
        
        # Insights Card (adjusted width)
        recommendations = insights.get('recommendations', [])
        insight_text = recommendations[0] if recommendations else "Select data to see insights"
        
        insights_card = html.Div([
            html.Div([
                html.H4("💡 Key Insight", style={'margin': '0', 'color': '#7c2d12', 'font-size': '0.9rem'}),
                html.P(insight_text, style={'margin': '10px 0', 'color': '#1f2937', 'font-size': '0.85rem', 'line-height': '1.4'})
            ], style={
                'background': 'linear-gradient(135deg, #fef7cd 0%, #fef3c7 100%)',
                'padding': '20px', 'border-radius': '12px', 'text-align': 'center',
                'box-shadow': '0 4px 6px rgba(0, 0, 0, 0.1)', 'border': '1px solid #f59e0b'
            })
        ], style={'flex': '1', 'min-width': '0'})
        
        cards = [best_mechanism_card, performance_card, throughput_card]
        if max_latency_card.children:  # Only add if max latency data exists
            cards.append(max_latency_card)
        cards.append(insights_card)
        
        return html.Div(cards, style={
            'display': 'flex',
            'flex-direction': 'row', 
            'justify-content': 'space-between',
            'align-items': 'stretch',
            'gap': '15px',
            'margin-bottom': '30px',
            'flex-wrap': 'nowrap'  # Prevent wrapping to ensure single row
        })

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
        """Render enhanced summary statistics tab with performance insights and better visual hierarchy."""
        logger.info(f"render_summary_tab called with: mechanisms={mechanisms}, message_sizes={message_sizes}")
        
        # Calculate percentiles from streaming data (much more robust!)
        percentile_df = self.calculate_percentiles_from_streaming(mechanisms, message_sizes)
        logger.info(f"Percentile df shape: {percentile_df.shape}, empty: {percentile_df.empty}")
        
        if percentile_df.empty:
            logger.warning("Percentile df is empty, returning no data message")
            return html.Div([
                html.H3("No Data Available"),
                html.P("No streaming data available for the selected filters. Try adjusting your filter selections.", 
                       style={'color': '#6b7280', 'fontSize': '1.1rem', 'textAlign': 'center', 'padding': '40px'})
            ], style={'textAlign': 'center', 'padding': '50px'})
        
        # Sort mechanisms by median one-way latency (best performing first)
        mechanism_order = (percentile_df[
            (percentile_df['latency_type'] == 'One-way') & 
            (percentile_df['percentile'] == 'P50 (Median)')
        ].groupby('mechanism')['latency_us'].mean().sort_values().index.tolist())
        
        # Calculate throughput data first for insights generation
        summary_df = self.filter_data(self.data_store['summary_data'], mechanisms, message_sizes)
        streaming_df = self.filter_data(self.data_store['streaming_data'], mechanisms, message_sizes)
        
        # Sort message sizes from smallest to largest for consistent ordering
        if not streaming_df.empty:
            message_size_order = sorted(streaming_df['message_size'].unique())
        else:
            message_size_order = []
        
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
        
        # Calculate max latency data for comparison
        max_latency_df = self.calculate_max_latencies(mechanisms, message_sizes)
        
        # Generate insights and summary cards
        insights = self.generate_performance_insights(percentile_df, throughput_df, max_latency_df)
        p50_comparison_matrix, max_comparison_matrix = self.create_performance_comparison_matrix(percentile_df, max_latency_df)
        summary_cards = self.create_summary_cards(insights, percentile_df, max_latency_df)
        
        # Create latency analysis data and charts
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
        
        # Add Percentile Statistics Table
        percentile_table_div = html.Div()
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
                style_table={'overflowX': 'auto'},
                style_cell={
                    'textAlign': 'left',
                    'padding': '10px',
                    'fontFamily': 'Arial, sans-serif',
                    'fontSize': '14px'
                },
                style_header={
                    'backgroundColor': '#f8f9fa',
                    'fontWeight': 'bold',
                    'color': '#495057'
                },
                style_data_conditional=[
                    {
                        'if': {'row_index': 'odd'},
                        'backgroundColor': '#f8f9fa'
                    }
                ],
                sort_action="native",
                filter_action="native",
                page_action="native",
                page_current=0,
                page_size=10,
            )
            
            percentile_table_div = html.Div([
                html.H3("📊 Dynamic Percentile Statistics"),
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
            
            # Common table style
            table_style = {
                'style_table': {'overflowX': 'auto', 'width': '100%'},
                'style_cell': {'textAlign': 'center', 'padding': '8px', 'fontFamily': 'Arial, sans-serif', 'fontSize': '12px'},
                'style_header': {'backgroundColor': '#f8f9fa', 'fontWeight': 'bold', 'color': '#495057'},
                'style_data_conditional': [{'if': {'row_index': 'odd'}, 'backgroundColor': '#f8f9fa'}],
                'sort_action': "native"
            }
            
            # Create individual table components
            tables = []
            
            # Max Latency Tables (side by side)
            if not max_oneway_pivot.empty or not max_roundtrip_pivot.empty:
                max_tables_row = html.Div([
                    html.H4("📈 Maximum Latency (μs)", style={'color': '#dc2626', 'margin-bottom': '15px', 'text-align': 'center'}),
                    html.Div([
                        # One-way max latency table
                        html.Div([
                            html.H5("One-way", style={'color': '#374151', 'margin-bottom': '10px', 'text-align': 'center'}),
                            dash_table.DataTable(
                                id='max-oneway-latency-table',
                                data=max_oneway_pivot.to_dict('records') if not max_oneway_pivot.empty else [],
                                columns=[{"name": i, "id": i} for i in max_oneway_pivot.columns] if not max_oneway_pivot.empty else [],
                                **table_style
                            ) if not max_oneway_pivot.empty else html.Div("No data available", style={'text-align': 'center', 'color': '#6b7280'})
                        ], style={'width': '48%', 'display': 'inline-block', 'margin-right': '4%'}),
                        
                        # Round-trip max latency table
                        html.Div([
                            html.H5("Round-trip", style={'color': '#374151', 'margin-bottom': '10px', 'text-align': 'center'}),
                            dash_table.DataTable(
                                id='max-roundtrip-latency-table',
                                data=max_roundtrip_pivot.to_dict('records') if not max_roundtrip_pivot.empty else [],
                                columns=[{"name": i, "id": i} for i in max_roundtrip_pivot.columns] if not max_roundtrip_pivot.empty else [],
                                **table_style
                            ) if not max_roundtrip_pivot.empty else html.Div("No data available", style={'text-align': 'center', 'color': '#6b7280'})
                        ], style={'width': '48%', 'display': 'inline-block'})
                    ], style={'margin-bottom': '30px'})
                ])
                tables.append(max_tables_row)
            
            # Average Latency Tables (side by side)
            if not avg_oneway_pivot.empty or not avg_roundtrip_pivot.empty:
                avg_tables_row = html.Div([
                    html.H4("📊 Average Latency (μs)", style={'color': '#059669', 'margin-bottom': '15px', 'text-align': 'center'}),
                    html.Div([
                        # One-way average latency table
                        html.Div([
                            html.H5("One-way", style={'color': '#374151', 'margin-bottom': '10px', 'text-align': 'center'}),
                            dash_table.DataTable(
                                id='avg-oneway-latency-table',
                                data=avg_oneway_pivot.to_dict('records') if not avg_oneway_pivot.empty else [],
                                columns=[{"name": i, "id": i} for i in avg_oneway_pivot.columns] if not avg_oneway_pivot.empty else [],
                                **table_style
                            ) if not avg_oneway_pivot.empty else html.Div("No data available", style={'text-align': 'center', 'color': '#6b7280'})
                        ], style={'width': '48%', 'display': 'inline-block', 'margin-right': '4%'}),
                        
                        # Round-trip average latency table
                        html.Div([
                            html.H5("Round-trip", style={'color': '#374151', 'margin-bottom': '10px', 'text-align': 'center'}),
                            dash_table.DataTable(
                                id='avg-roundtrip-latency-table',
                                data=avg_roundtrip_pivot.to_dict('records') if not avg_roundtrip_pivot.empty else [],
                                columns=[{"name": i, "id": i} for i in avg_roundtrip_pivot.columns] if not avg_roundtrip_pivot.empty else [],
                                **table_style
                            ) if not avg_roundtrip_pivot.empty else html.Div("No data available", style={'text-align': 'center', 'color': '#6b7280'})
                        ], style={'width': '48%', 'display': 'inline-block'})
                    ])
                ])
                tables.append(avg_tables_row)
            
            latency_table_div = html.Div([
                html.H3("📊 Latency Performance Data"),
                html.P(f"Latency measurements organized by message size and mechanism. Lower values indicate better performance.",
                       style={'color': '#6b7280', 'margin-bottom': '20px', 'fontSize': '0.95rem'}),
                *tables
            ], className="dash-table-container")
        
        if throughput_data:
            # Create 2 separate throughput tables: One-way and Round-trip
            throughput_df = pd.DataFrame(throughput_data)
            
            # Filter data for each table type
            oneway_throughput_data = throughput_df[throughput_df['type'] == 'One-way']
            roundtrip_throughput_data = throughput_df[throughput_df['type'] == 'Round-trip']
            
            # Create pivot tables for messages/sec
            oneway_msgs_pivot = self.create_pivot_table(oneway_throughput_data.to_dict('records'), 'msgs_per_sec', round_digits=3)
            roundtrip_msgs_pivot = self.create_pivot_table(roundtrip_throughput_data.to_dict('records'), 'msgs_per_sec', round_digits=3)
            
            # Create pivot tables for bytes/sec (convert to MB/s for readability)
            oneway_throughput_mb = oneway_throughput_data.copy()
            roundtrip_throughput_mb = roundtrip_throughput_data.copy()
            if not oneway_throughput_mb.empty:
                oneway_throughput_mb['bytes_per_sec'] = oneway_throughput_mb['bytes_per_sec'] / 1_000_000  # Convert to MB/s
            if not roundtrip_throughput_mb.empty:
                roundtrip_throughput_mb['bytes_per_sec'] = roundtrip_throughput_mb['bytes_per_sec'] / 1_000_000  # Convert to MB/s
                
            oneway_bytes_pivot = self.create_pivot_table(oneway_throughput_mb.to_dict('records'), 'bytes_per_sec', round_digits=1)
            roundtrip_bytes_pivot = self.create_pivot_table(roundtrip_throughput_mb.to_dict('records'), 'bytes_per_sec', round_digits=1)
            
            # Common table style for throughput tables
            throughput_table_style = {
                'style_table': {'overflowX': 'auto', 'width': '100%'},
                'style_cell': {'textAlign': 'center', 'padding': '8px', 'fontFamily': 'Arial, sans-serif', 'fontSize': '12px'},
                'style_header': {'backgroundColor': '#e0f2fe', 'fontWeight': 'bold', 'color': '#0277bd'},
                'style_data_conditional': [{'if': {'row_index': 'odd'}, 'backgroundColor': '#f8f9fa'}],
                'sort_action': "native"
            }
            
            throughput_tables = []
            
            # Messages/sec Tables (side by side)
            if not oneway_msgs_pivot.empty or not roundtrip_msgs_pivot.empty:
                msgs_tables_row = html.Div([
                    html.H4("📊 Message Throughput (messages/sec)", style={'color': '#0277bd', 'margin-bottom': '15px', 'text-align': 'center'}),
                    html.Div([
                        # One-way messages/sec table
                        html.Div([
                            html.H5("One-way", style={'color': '#374151', 'margin-bottom': '10px', 'text-align': 'center'}),
                            dash_table.DataTable(
                                id='oneway-msgs-throughput-table',
                                data=oneway_msgs_pivot.to_dict('records') if not oneway_msgs_pivot.empty else [],
                                columns=[{"name": i, "id": i} for i in oneway_msgs_pivot.columns] if not oneway_msgs_pivot.empty else [],
                                **throughput_table_style
                            ) if not oneway_msgs_pivot.empty else html.Div("No data available", style={'text-align': 'center', 'color': '#6b7280'})
                        ], style={'width': '48%', 'display': 'inline-block', 'margin-right': '4%'}),
                        
                        # Round-trip messages/sec table
                        html.Div([
                            html.H5("Round-trip", style={'color': '#374151', 'margin-bottom': '10px', 'text-align': 'center'}),
                            dash_table.DataTable(
                                id='roundtrip-msgs-throughput-table',
                                data=roundtrip_msgs_pivot.to_dict('records') if not roundtrip_msgs_pivot.empty else [],
                                columns=[{"name": i, "id": i} for i in roundtrip_msgs_pivot.columns] if not roundtrip_msgs_pivot.empty else [],
                                **throughput_table_style
                            ) if not roundtrip_msgs_pivot.empty else html.Div("No data available", style={'text-align': 'center', 'color': '#6b7280'})
                        ], style={'width': '48%', 'display': 'inline-block'})
                    ], style={'margin-bottom': '30px'})
                ])
                throughput_tables.append(msgs_tables_row)
            
            # Bytes/sec Tables (side by side)
            if not oneway_bytes_pivot.empty or not roundtrip_bytes_pivot.empty:
                bytes_tables_row = html.Div([
                    html.H4("🚀 Data Throughput (MB/sec)", style={'color': '#0277bd', 'margin-bottom': '15px', 'text-align': 'center'}),
                    html.Div([
                        # One-way bytes/sec table
                        html.Div([
                            html.H5("One-way", style={'color': '#374151', 'margin-bottom': '10px', 'text-align': 'center'}),
                            dash_table.DataTable(
                                id='oneway-bytes-throughput-table',
                                data=oneway_bytes_pivot.to_dict('records') if not oneway_bytes_pivot.empty else [],
                                columns=[{"name": i, "id": i} for i in oneway_bytes_pivot.columns] if not oneway_bytes_pivot.empty else [],
                                **throughput_table_style
                            ) if not oneway_bytes_pivot.empty else html.Div("No data available", style={'text-align': 'center', 'color': '#6b7280'})
                        ], style={'width': '48%', 'display': 'inline-block', 'margin-right': '4%'}),
                        
                        # Round-trip bytes/sec table
                        html.Div([
                            html.H5("Round-trip", style={'color': '#374151', 'margin-bottom': '10px', 'text-align': 'center'}),
                            dash_table.DataTable(
                                id='roundtrip-bytes-throughput-table',
                                data=roundtrip_bytes_pivot.to_dict('records') if not roundtrip_bytes_pivot.empty else [],
                                columns=[{"name": i, "id": i} for i in roundtrip_bytes_pivot.columns] if not roundtrip_bytes_pivot.empty else [],
                                **throughput_table_style
                            ) if not roundtrip_bytes_pivot.empty else html.Div("No data available", style={'text-align': 'center', 'color': '#6b7280'})
                        ], style={'width': '48%', 'display': 'inline-block'})
                    ])
                ])
                throughput_tables.append(bytes_tables_row)
            
            throughput_table_div = html.Div([
                html.H3("🚀 Throughput Performance Data"),
                html.P(f"Throughput measurements organized by message size and mechanism. Higher values indicate better performance.",
                       style={'color': '#6b7280', 'margin-bottom': '20px', 'fontSize': '0.95rem'}),
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
                    style_table={'overflowX': 'auto'},
                    style_cell={'textAlign': 'center', 'padding': '12px', 'fontFamily': 'Arial, sans-serif', 'fontSize': '14px'},
                    style_header={'backgroundColor': '#e0f2fe', 'fontWeight': 'bold', 'color': '#0277bd'},
                    style_data_conditional=[
                        {'if': {'row_index': 'odd'}, 'backgroundColor': '#f8f9fa'},
                        {'if': {'column_id': 'mechanism'}, 'textAlign': 'left', 'fontWeight': 'bold'},
                        {'if': {'column_id': 'Overall Score'}, 'backgroundColor': '#fff3cd', 'fontWeight': 'bold'},
                    ],
                    sort_action="native",
                )
                
                matrix_tables.append(html.Div([
                    html.H4("📊 P50 Latency Comparison", style={'color': '#1565c0', 'margin-bottom': '10px'}),
                    html.P("Relative P50 latency performance. Lower percentages = better typical performance.",
                           style={'color': '#6b7280', 'margin-bottom': '15px', 'fontSize': '0.9rem'}),
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
                    style_table={'overflowX': 'auto'},
                    style_cell={'textAlign': 'center', 'padding': '12px', 'fontFamily': 'Arial, sans-serif', 'fontSize': '14px'},
                    style_header={'backgroundColor': '#fee2e2', 'fontWeight': 'bold', 'color': '#b91c1c'},
                    style_data_conditional=[
                        {'if': {'row_index': 'odd'}, 'backgroundColor': '#f8f9fa'},
                        {'if': {'column_id': 'mechanism'}, 'textAlign': 'left', 'fontWeight': 'bold'},
                        {'if': {'column_id': 'Overall Score'}, 'backgroundColor': '#fed7d7', 'fontWeight': 'bold'},
                    ],
                    sort_action="native",
                )
                
                matrix_tables.append(html.Div([
                    html.H4("⚡ Max Latency Comparison", style={'color': '#b91c1c', 'margin-bottom': '10px'}),
                    html.P("Relative maximum latency performance. Lower percentages = better worst-case performance.",
                           style={'color': '#6b7280', 'margin-bottom': '15px', 'fontSize': '0.9rem'}),
                    max_table
                ], style={'margin-bottom': '20px'}))
            
            comparison_matrix_div = html.Div([
                html.H3("🏁 Performance Comparison Matrix", style={'color': '#1565c0', 'margin-bottom': '15px'}),
                html.P("Comprehensive performance comparison across latency metrics. Lower percentages indicate better performance.",
                       style={'color': '#6b7280', 'margin-bottom': '25px', 'fontSize': '0.95rem'}),
                html.Div(matrix_tables)
            ], className="dash-table-container")

        # Organize layout into logical sections
        return html.Div([
            # === OVERVIEW SECTION ===
            html.Div([
                html.H2("📈 Performance Overview", style={'color': '#1f2937', 'margin-bottom': '20px', 'font-size': '1.5rem'}),
                summary_cards
            ], style={'margin-bottom': '40px'}),
            
            # === PERFORMANCE COMPARISON SECTION ===
            html.Div([
                html.H2("🏁 Head-to-Head Comparison", style={'color': '#1f2937', 'margin-bottom': '20px', 'font-size': '1.5rem'}),
                comparison_matrix_div
            ], style={'margin-bottom': '40px'}),
            
            # === INSIGHTS SECTION ===
            html.Div([
                html.H2("💡 Performance Insights", style={'color': '#1f2937', 'margin-bottom': '20px', 'font-size': '1.5rem'}),
                html.Div([
                    html.Div([
                        html.H4("🎯 Recommendations:", style={'color': '#059669', 'margin-bottom': '15px'}),
                        html.Ul([
                            html.Li(rec, style={'margin-bottom': '8px', 'color': '#374151'}) 
                            for rec in insights.get('recommendations', ['Select data to see recommendations'])
                        ], style={'padding-left': '20px'})
                    ], style={
                        'background': 'linear-gradient(135deg, #f0fdf4 0%, #dcfce7 100%)',
                        'padding': '25px', 'border-radius': '12px',
                        'box-shadow': '0 4px 6px rgba(0, 0, 0, 0.1)', 'border': '1px solid #10b981'
                    })
                ], className="chart-container")
            ], style={'margin-bottom': '40px'}),
            
            # === LATENCY ANALYSIS SECTION ===
            html.Div([
                html.H2("📊 Latency Analysis", style={'color': '#1f2937', 'margin-bottom': '20px', 'font-size': '1.5rem'}),
            html.Div([
                dcc.Graph(id='avg-latency-chart', figure=avg_latency_fig)
            ], className="chart-container"),
            html.Div([
                dcc.Graph(id='max-latency-chart', figure=max_latency_fig)
            ], className="chart-container"),
                latency_table_div if latency_data else html.Div()
            ], style={'margin-bottom': '40px'}),
            
            # === THROUGHPUT ANALYSIS SECTION ===
            html.Div([
                html.H2("🚀 Throughput Analysis", style={'color': '#1f2937', 'margin-bottom': '20px', 'font-size': '1.5rem'}),
            html.Div([
                dcc.Graph(id='throughput-msgs-chart', figure=msgs_fig)
            ], className="chart-container"),
            html.Div([
                dcc.Graph(id='throughput-bytes-chart', figure=bytes_fig)
            ], className="chart-container"),
                throughput_table_div if throughput_data else html.Div()
            ], style={'margin-bottom': '40px'})
        ])
    
    def render_timeseries_tab(self, mechanisms: List, message_sizes: List):
        """Render time series analysis tab."""
        streaming_df = self.filter_data(self.data_store['streaming_data'], mechanisms, message_sizes)
        
        if streaming_df.empty:
            return html.Div([
                html.H3("No Data Available"),
                html.P("No streaming data available for the selected filters. Try adjusting your filter selections.", 
                       style={'color': '#6b7280', 'fontSize': '1.1rem', 'textAlign': 'center', 'padding': '40px'})
            ], style={'textAlign': 'center', 'padding': '50px'})
            
        # Create time series controls section
        available_mechanisms = mechanisms if mechanisms and len(mechanisms) > 0 else streaming_df['mechanism'].unique().tolist()
        
        controls_section = html.Div([
            html.H3("Time Series Controls", style={'margin-bottom': '20px'}),
            
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
                ], style={'width': '30%', 'display': 'inline-block', 'vertical-align': 'top', 'margin-left': '3%'}),
            
            html.Div([
                    html.Label("Line & Dot Sizes:", style={'margin-bottom': '10px', 'display': 'block'}),
                html.Div([
                        html.Label("Dot Size:", style={'margin-bottom': '5px', 'display': 'block', 'font-size': '0.85rem'}),
                        dcc.Slider(
                            id='ts-dot-size-slider',
                            min=1,
                            max=10,
                            step=1,
                            value=3,
                            marks={i: str(i) for i in [1, 3, 5, 10]},
                            tooltip={"placement": "bottom", "always_visible": True}
                        ),
                    ], style={'margin-bottom': '15px'}),
                    html.Div([
                        html.Label("Line Width:", style={'margin-bottom': '5px', 'display': 'block', 'font-size': '0.85rem'}),
                        dcc.Slider(
                            id='ts-line-width-slider',
                            min=1,
                            max=8,
                            step=1,
                            value=2,
                            marks={i: str(i) for i in [1, 2, 4, 8]},
                            tooltip={"placement": "bottom", "always_visible": True}
                        ),
                    ]),
                ], style={'width': '30%', 'display': 'inline-block', 'vertical-align': 'top', 'margin-left': '3%'}),
            ], style={'margin-bottom': '30px'}),
            
            # Threshold controls for each mechanism (dynamically generated)
            html.Div(id='threshold-controls-container', children=[
                html.Label("Outlier Thresholds (μs):", style={'margin-bottom': '15px', 'display': 'block', 'font-weight': 'bold'}),
                html.P("Thresholds will appear here based on your filter selections.", 
                       style={'color': '#6b7280', 'font-size': '0.9rem', 'margin-bottom': '15px', 'font-style': 'italic'}),
            ], style={'margin-bottom': '30px'}),
            
            # Charts container that will be updated
            html.Div(id='ts-charts-container')
        ], className="filter-section", style={'margin-bottom': '30px'})
        
        return controls_section
    
    def render_timeseries_charts(self, mechanisms: List, message_sizes: List, 
                                moving_avg_window: int, display_options: List, thresholds: dict = None, 
                                dot_size: int = 3, line_width: int = 2):
        """Render the actual time series charts with outlier detection."""
        streaming_df = self.filter_data(self.data_store['streaming_data'], mechanisms, message_sizes)
        
        if streaming_df.empty:
            return html.Div([
                html.P("No data available for current selections.", 
                       style={'textAlign': 'center', 'color': '#6b7280', 'fontSize': '1.1rem', 'padding': '40px'})
            ])

        # Performance note for downsampling
        MAX_POINTS_PER_RUN = 10000
        total_original_points = len(streaming_df)
        performance_note = html.Div()
        if total_original_points > MAX_POINTS_PER_RUN:
            performance_note = html.Div([
                html.P(f"Performance Note: Displaying sampled data for better interactivity. "
                       f"Original dataset: {total_original_points:,} points, "
                       f"Displaying: ~{min(MAX_POINTS_PER_RUN, total_original_points):,} points per run.")
            ], className="performance-note")

        # Create separate graph for each mechanism
        mechanism_graphs = []
        
        # Color scheme: consistent colors for latency types
        ONEWAY_COLOR = '#3b82f6'      # Blue for one-way latency
        ROUNDTRIP_COLOR = '#f59e0b'   # Orange for round-trip latency
        OUTLIER_COLOR = '#ef4444'     # Red for outliers
        
        # Symbol scheme: different symbols for each mechanism
        symbols = ['circle', 'square', 'diamond', 'cross', 'x', 'triangle-up', 'triangle-down', 'star']
        
        for i, mechanism in enumerate(streaming_df['mechanism'].unique()):
            symbol = symbols[i % len(symbols)]
            fig = go.Figure()
            
            # Filter data for this mechanism
            mechanism_df = streaming_df[streaming_df['mechanism'] == mechanism]
            
            if len(mechanism_df) > 0:
                # Process all data for this mechanism
                run_data = mechanism_df.copy()
                
                # Downsample if necessary
                if len(run_data) > MAX_POINTS_PER_RUN:
                    step_size = len(run_data) // MAX_POINTS_PER_RUN
                    logging.info(f"Downsampled {mechanism} from {len(run_data)} to {len(run_data.iloc[::step_size])} points")
                    run_data = run_data.iloc[::step_size].copy()
                
                # Reset index to create sequential index for x-axis
                run_data = run_data.reset_index(drop=True)
                    
                # Get threshold for this mechanism
                threshold = thresholds.get(mechanism) if thresholds else None
                
                if 'raw_dots' in display_options:
                    # ONE-WAY LATENCY POINTS
                    if threshold is not None and threshold > 0:
                        normal_data = run_data[run_data['one_way_latency_us'] <= threshold]
                        outlier_data = run_data[run_data['one_way_latency_us'] > threshold]
                        
                        # Add normal one-way points (BLUE)
                        if len(normal_data) > 0:
                            fig.add_trace(go.Scatter(
                                x=normal_data.index,
                                y=normal_data['one_way_latency_us'],
                                mode='markers',
                                name=f'One-way ({mechanism})',
                                marker=dict(color=ONEWAY_COLOR, size=dot_size, opacity=0.7, symbol=symbol),
                                legendgroup='oneway',
                                showlegend=True
                            ))
                        
                        # Add one-way outlier points (RED)
                        if len(outlier_data) > 0:
                            fig.add_trace(go.Scatter(
                                x=outlier_data.index,
                                y=outlier_data['one_way_latency_us'],
                                mode='markers',
                                name=f'One-way Outliers ({mechanism}, >{threshold}μs)',
                                marker=dict(color=OUTLIER_COLOR, size=dot_size + 2, opacity=0.9, symbol='x', 
                                          line=dict(width=line_width, color='darkred')),
                                legendgroup='oneway-outlier',
                                showlegend=True
                            ))
                    else:
                        # No threshold set, show all one-way points normally (BLUE)
                        fig.add_trace(go.Scatter(
                            x=run_data.index,
                            y=run_data['one_way_latency_us'],
                            mode='markers',
                            name=f'One-way ({mechanism})',
                            marker=dict(color=ONEWAY_COLOR, size=dot_size, opacity=0.7, symbol=symbol),
                            legendgroup='oneway',
                            showlegend=True
                        ))
                        
                    # ROUND-TRIP LATENCY POINTS
                    if 'round_trip_latency_us' in run_data.columns:
                        if threshold is not None and threshold > 0:
                            normal_rt = run_data[run_data['round_trip_latency_us'] <= threshold]
                            outlier_rt = run_data[run_data['round_trip_latency_us'] > threshold]
                            
                            # Normal round-trip points (ORANGE)
                            if len(normal_rt) > 0:
                                fig.add_trace(go.Scatter(
                                    x=normal_rt.index,
                                    y=normal_rt['round_trip_latency_us'],
                                    mode='markers',
                                    name=f'Round-trip ({mechanism})',
                                    marker=dict(color=ROUNDTRIP_COLOR, size=dot_size, opacity=0.7, symbol=symbol),
                                    legendgroup='roundtrip',
                                    showlegend=True
                                ))
                            
                            # Round-trip outlier points (RED)
                            if len(outlier_rt) > 0:
                                fig.add_trace(go.Scatter(
                                    x=outlier_rt.index,
                                    y=outlier_rt['round_trip_latency_us'],
                                    mode='markers',
                                    name=f'Round-trip Outliers ({mechanism}, >{threshold}μs)',
                                    marker=dict(color=OUTLIER_COLOR, size=dot_size + 2, opacity=0.9, symbol='x',
                                              line=dict(width=line_width, color='darkred')),
                                    legendgroup='roundtrip-outlier',
                                    showlegend=True
                                ))
                        else:
                            # No threshold, show all round-trip points normally (ORANGE)
                            fig.add_trace(go.Scatter(
                                x=run_data.index,
                                y=run_data['round_trip_latency_us'],
                                mode='markers',
                                name=f'Round-trip ({mechanism})',
                                marker=dict(color=ROUNDTRIP_COLOR, size=dot_size, opacity=0.7, symbol=symbol),
                                legendgroup='roundtrip',
                                showlegend=True
                            ))
                    
                if 'moving_avg' in display_options and len(run_data) > moving_avg_window:
                    # Calculate moving average (data is already sorted by index)
                    run_data['one_way_ma'] = run_data['one_way_latency_us'].rolling(window=moving_avg_window, min_periods=1).mean()
                    if 'round_trip_latency_us' in run_data.columns:
                        run_data['round_trip_ma'] = run_data['round_trip_latency_us'].rolling(window=moving_avg_window, min_periods=1).mean()
                    
                    # Add moving average traces with consistent color coding
                    fig.add_trace(go.Scatter(
                        x=run_data.index,
                        y=run_data['one_way_ma'],
                        mode='lines',
                        name=f'One-way MA ({mechanism})',
                        line=dict(color=ONEWAY_COLOR, width=line_width, dash='solid'),
                        opacity=0.9,
                        legendgroup='oneway-ma',
                        showlegend=True
                    ))
                    
                    if 'round_trip_latency_us' in run_data.columns:
                        fig.add_trace(go.Scatter(
                            x=run_data.index,
                            y=run_data['round_trip_ma'],
                            mode='lines',
                            name=f'Round-trip MA ({mechanism})',
                            line=dict(color=ROUNDTRIP_COLOR, width=line_width, dash='solid'),
                            opacity=0.9,
                            legendgroup='roundtrip-ma',
                            showlegend=True
                        ))
            
            fig.update_layout(
                title=f"Latency Time Series - {mechanism}",
                xaxis_title="Sample Index", 
                yaxis_title="Latency (μs)",
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
            
            mechanism_graphs.append(html.Div([dcc.Graph(figure=fig)], className="chart-container"))
        
        return html.Div([
            performance_note,
            *mechanism_graphs
        ])
    
    def render_throughput_tab(self, mechanisms: List, message_sizes: List,
                             moving_avg_window: int, display_options: List):
        """Render throughput trends tab."""
        # For now, use summary data if available
        summary_df = self.filter_data(self.data_store['summary_data'], mechanisms, message_sizes)
        
        if summary_df.empty:
            return html.Div("No throughput data available for selected filters.")
        
        # Create throughput comparison charts
        throughput_data = []
        for _, row in summary_df.iterrows():
            if not pd.isna(row['one_way_msgs_per_sec']):
                throughput_data.append({
                    'mechanism': row['mechanism'],
                    'message_size': row['message_size'],
                    'type': 'One-way',
                    'msgs_per_sec': row['one_way_msgs_per_sec'],
                    'mbps': row['one_way_bytes_per_sec'] / 1_000_000 if not pd.isna(row['one_way_bytes_per_sec']) else 0
                })
            if not pd.isna(row['round_trip_msgs_per_sec']):
                throughput_data.append({
                    'mechanism': row['mechanism'],
                    'message_size': row['message_size'],
                    'type': 'Round-trip',
                    'msgs_per_sec': row['round_trip_msgs_per_sec'],
                    'mbps': row['round_trip_bytes_per_sec'] / 1_000_000 if not pd.isna(row['round_trip_bytes_per_sec']) else 0
                })
        
        if not throughput_data:
            return html.Div("No throughput data available.")
        
        throughput_df = pd.DataFrame(throughput_data)
        
        # Messages per second chart
        msgs_fig = px.scatter(
            throughput_df,
            x='message_size',
            y='msgs_per_sec',
            color='mechanism',
            symbol='type',
            title="Throughput: Messages per Second vs Message Size",
            labels={'msgs_per_sec': 'Messages per Second', 'message_size': 'Message Size (bytes)'},
            hover_data=['mechanism']
        )
        
        # Megabits per second chart
        mbps_fig = px.scatter(
            throughput_df,
            x='message_size',
            y='mbps',
            color='mechanism',
            symbol='type',
            title="Throughput: Megabits per Second vs Message Size",
            labels={'mbps': 'Megabits per Second', 'message_size': 'Message Size (bytes)'},
            hover_data=['mechanism']
        )
        
        # Add Aggregated Statistics Table
        agg_table_div = html.Div()
        streaming_df = self.filter_data(self.data_store['streaming_data'], mechanisms, message_sizes)
        
        if not streaming_df.empty:
            # Create aggregated statistics by mechanism and message size
            agg_stats = streaming_df.groupby(['mechanism', 'message_size']).agg({
                'one_way_latency_us': ['count', 'mean', 'median', 'std', 'min', 'max'],
                'round_trip_latency_us': ['count', 'mean', 'median', 'std', 'min', 'max']
            }).round(2)
            
            # Flatten column names
            agg_stats.columns = [f"{col[1]}_{col[0]}" for col in agg_stats.columns]
            agg_stats = agg_stats.reset_index()
            
            # Rename columns for better display
            agg_column_mapping = {
                'mechanism': 'Mechanism',
                'message_size': 'Message Size (bytes)',
                'count_one_way_latency_us': 'One-way Count',
                'mean_one_way_latency_us': 'One-way Mean (μs)',
                'median_one_way_latency_us': 'One-way Median (μs)',
                'std_one_way_latency_us': 'One-way Std Dev (μs)',
                'min_one_way_latency_us': 'One-way Min (μs)',
                'max_one_way_latency_us': 'One-way Max (μs)',
                'count_round_trip_latency_us': 'Round-trip Count',
                'mean_round_trip_latency_us': 'Round-trip Mean (μs)',
                'median_round_trip_latency_us': 'Round-trip Median (μs)',
                'std_round_trip_latency_us': 'Round-trip Std Dev (μs)',
                'min_round_trip_latency_us': 'Round-trip Min (μs)',
                'max_round_trip_latency_us': 'Round-trip Max (μs)'
            }
            agg_stats = agg_stats.rename(columns=agg_column_mapping)
            
            agg_table = dash_table.DataTable(
                id='aggregated-table',
                data=agg_stats.to_dict('records'),
                columns=[{"name": i, "id": i} for i in agg_stats.columns],
                style_table={'overflowX': 'auto'},
                style_cell={
                    'textAlign': 'left',
                    'padding': '10px',
                    'fontFamily': 'Arial, sans-serif',
                    'fontSize': '14px'
                },
                style_header={
                    'backgroundColor': '#e8f5e8',
                    'fontWeight': 'bold',
                    'color': '#2e7d32'
                },
                style_data_conditional=[
                    {
                        'if': {'row_index': 'odd'},
                        'backgroundColor': '#f8f9fa'
                    }
                ],
                sort_action="native",
                filter_action="native",
                page_action="native",
                page_current=0,
                page_size=10,
            )
            
            agg_table_div = html.Div([
                html.H3("📈 Performance Statistics by Configuration"),
                html.P("Statistical summary grouped by mechanism and message size including count, mean, median, standard deviation, min, and max values. "
                       "Use this data to understand throughput capabilities and latency distributions.",
                       style={'color': '#6b7280', 'margin-bottom': '20px', 'fontSize': '0.95rem'}),
                agg_table
            ], className="dash-table-container")

        return html.Div([
            html.Div([
                dcc.Graph(figure=msgs_fig)
            ], className="chart-container"),
            html.Div([
                dcc.Graph(figure=mbps_fig)
            ], className="chart-container"),
            agg_table_div
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
app_css = """
/* Import modern fonts */
@import url('https://fonts.googleapis.com/css2?family=Inter:wght@300;400;500;600;700&display=swap');

/* Global styles */
* {
    font-family: 'Inter', -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
}

body {
    margin: 0;
    background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
    min-height: 100vh;
}

.header-title {
    text-align: center;
    color: #ffffff;
    margin: 0 0 30px 0;
    font-size: 2.5rem;
    font-weight: 700;
    text-shadow: 0 2px 4px rgba(0,0,0,0.1);
    background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
    padding: 30px 0;
    margin: -20px -20px 30px -20px;
    border-radius: 0 0 20px 20px;
    box-shadow: 0 4px 20px rgba(0,0,0,0.1);
}

.dashboard-container {
    display: flex;
    height: 100vh;
    background: #f8fafc;
}

.sidebar {
    width: 240px;
    padding: 16px;
    background: rgba(255, 255, 255, 0.95);
    backdrop-filter: blur(10px);
    border-right: none;
    box-shadow: 4px 0 20px rgba(0,0,0,0.08);
    overflow-y: auto;
    border-radius: 0 20px 20px 0;
    margin: 20px 0 20px 20px;
}

.main-content {
    flex: 1;
    padding: 25px;
    overflow-y: auto;
    background: #f8fafc;
}

/* Filter sections */
.filter-section {
    background: white;
    border-radius: 16px;
    padding: 20px;
    margin-bottom: 20px;
    box-shadow: 0 4px 15px rgba(0,0,0,0.08);
    border: 1px solid rgba(255,255,255,0.2);
}

.filter-checklist {
    margin-bottom: 15px;
}

.filter-checklist label {
    font-weight: 500;
    color: #374151;
    margin-bottom: 12px;
    display: block;
    font-size: 0.9rem;
}

/* Slider styling */
.slider {
    margin-bottom: 20px;
}

.slider .rc-slider {
    border-radius: 8px;
}

.slider .rc-slider-track {
    background: linear-gradient(90deg, #667eea 0%, #764ba2 100%);
    border-radius: 8px;
    height: 6px;
}

.slider .rc-slider-handle {
    background: white;
    border: 3px solid #667eea;
    width: 20px;
    height: 20px;
    margin-top: -7px;
    box-shadow: 0 2px 8px rgba(0,0,0,0.15);
}

/* Tab styling */
.tab-content {
    background: white;
    border-radius: 20px;
    padding: 30px;
    box-shadow: 0 8px 32px rgba(0,0,0,0.12);
    margin-bottom: 20px;
    border: 1px solid rgba(255,255,255,0.2);
}

/* Chart containers */
.chart-container {
    background: white;
    border-radius: 16px;
    padding: 25px;
    margin-bottom: 25px;
    box-shadow: 0 4px 20px rgba(0,0,0,0.08);
    border: 1px solid rgba(255,255,255,0.2);
    transition: transform 0.2s ease, box-shadow 0.2s ease;
}

.chart-container:hover {
    transform: translateY(-2px);
    box-shadow: 0 8px 30px rgba(0,0,0,0.12);
}

/* Table styling */
.dash-table-container {
    background: white;
    border-radius: 16px;
    padding: 20px;
    margin: 25px 0;
    box-shadow: 0 4px 20px rgba(0,0,0,0.08);
    border: 1px solid rgba(255,255,255,0.2);
}

.dash-table-container .dash-spreadsheet-container {
    border-radius: 12px;
    overflow: hidden;
}

.dash-table-container .dash-spreadsheet-container .dash-spreadsheet-inner table {
    border-radius: 12px;
}

/* Performance note styling */
.performance-note {
    background: linear-gradient(135deg, #fef7cd 0%, #fef3c7 100%);
    border: 1px solid #f59e0b;
    border-radius: 12px;
    padding: 16px 20px;
    margin-bottom: 25px;
    box-shadow: 0 2px 8px rgba(245, 158, 11, 0.1);
}

.performance-note p {
    margin: 0;
    color: #92400e;
    font-weight: 500;
}

/* Section headers */
h3 {
    color: #1f2937;
    font-weight: 600;
    font-size: 1.5rem;
    margin-bottom: 16px;
    display: flex;
    align-items: center;
    gap: 8px;
}

/* Labels */
label {
    color: #374151;
    font-weight: 500;
    font-size: 0.9rem;
    margin-bottom: 8px;
    display: block;
}

/* Checkboxes and inputs */
input[type="checkbox"] {
    accent-color: #667eea;
    transform: scale(1.1);
    margin-right: 8px;
}

/* Scrollbar styling */
::-webkit-scrollbar {
    width: 8px;
}

::-webkit-scrollbar-track {
    background: #f1f5f9;
    border-radius: 4px;
}

::-webkit-scrollbar-thumb {
    background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
    border-radius: 4px;
}

::-webkit-scrollbar-thumb:hover {
    background: linear-gradient(135deg, #5a6fd8 0%, #6b4190 100%);
}

/* Modern card effect for graphs */
._dash-graph {
    border-radius: 12px;
    overflow: hidden;
}

/* Dark mode toggle styling */
.dark-mode-toggle input[type="checkbox"] {
    appearance: none;
    width: 50px;
    height: 25px;
    border-radius: 25px;
    background: #ccc;
    position: relative;
    outline: none;
    cursor: pointer;
    transition: background 0.3s;
}

.dark-mode-toggle input[type="checkbox"]:checked {
    background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
}

.dark-mode-toggle input[type="checkbox"]:before {
    content: '';
    position: absolute;
    top: 2px;
    left: 2px;
    width: 21px;
    height: 21px;
    border-radius: 50%;
    background: white;
    transition: transform 0.3s;
    box-shadow: 0 2px 4px rgba(0,0,0,0.2);
}

.dark-mode-toggle input[type="checkbox"]:checked:before {
    transform: translateX(25px);
}


/* Responsive adjustments */
@media (max-width: 1200px) {
    .sidebar {
        width: 280px;
    }
}

@media (max-width: 768px) {
    .dashboard-container {
        flex-direction: column;
    }
    
    .sidebar {
        width: 100%;
        border-radius: 20px 20px 0 0;
        margin: 20px 20px 0 20px;
    }
    
    .main-content {
        padding: 20px;
    }
}
"""

if __name__ == "__main__":
    exit(main())
