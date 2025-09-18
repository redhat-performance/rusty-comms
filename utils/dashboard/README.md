# Rusty-Comms Performance Dashboard

A high-performance interactive web dashboard for analyzing Inter-Process Communication (IPC) benchmark results. Features modular architecture, advanced visualizations, and intelligent performance analysis.

## 🚀 **Quick Start**

```bash
# Install dependencies
pip install -r requirements.txt

# Start dashboard
python dashboard.py --dir /path/to/benchmark/data --host 0.0.0.0 --port 8050
```

Access at: `http://localhost:8050`

## 📊 **Key Features**

### **Summary Analysis**
- **Performance Overview**: AI-generated insights and key metrics cards
- **Head-to-Head Comparisons**: Interactive comparison matrices for P50/Max latency
- **Statistical Analysis**: Comprehensive latency and throughput breakdowns
- **Data Tables**: Sortable pivot tables for one-way vs round-trip performance

### **Time Series Analysis** 
- **Interactive Visualizations**: Real-time scatter plots with advanced controls
- **5 Preset Configurations**: Performance, detailed inspection, statistical, outlier detection, and reset modes
- **Statistical Overlays**: Moving averages, percentile bands, anomaly detection
- **Anomaly Detection**: ML-based (Isolation Forest) and statistical (IQR) methods
- **Smart Sampling**: Multiple strategies for large datasets (8M+ records)

### **Advanced Features**
- **Interactive File Browser**: Rich file explorer replacing simple dropdown
- **Threaded Processing**: Concurrent loading with 8 workers for optimal performance  
- **Enhanced Caching**: TTL-based cache with LRU eviction (10min TTL, 50 items)
- **Professional UI**: Dark theme with neon accents and responsive design

## 🏗️ **Architecture**

### **Modular Design**
- **`cache.py`** - Caching layer with decorators and TTL management
- **`data_processor.py`** - Data discovery, loading, and processing logic
- **`dashboard.py`** - Main UI application and visualization components
- **`dashboard_styles.css`** - External CSS styling

### **Performance Optimizations**
- **Concurrent Processing**: ThreadPoolExecutor for file loading (8 summary, 6 streaming workers)
- **Memory Efficient**: Handles millions of records with pandas optimization
- **Smart Caching**: 70-90% cache hit rate for repeated analyses
- **Data Sampling**: Adaptive strategies for large dataset visualization

## 📈 **Performance Metrics**

| Operation | Time | Details |
|-----------|------|---------|
| **Startup** | 5-15s | Depends on data size |
| **Summary Analysis** | 8-12s | Full statistical processing |
| **Time Series** | 3-8s | Varies by sampling strategy |
| **Memory Usage** | 200-500MB | For typical datasets |

## 📁 **Data Format**

### **Auto-Discovery**
The dashboard automatically discovers files in your data directory:

- **Summary**: `*_results.json` - Statistical summaries and throughput data
- **Streaming**: `*_streaming.json` or `*.csv` - Raw latency measurements
- **Mixed Formats**: Supports both JSON and CSV in the same directory

### **Sample Structure**
```json
{
  "mechanism": "SharedMemory",
  "message_size": 1024,
  "one_way_latency_us": 1.23,
  "round_trip_latency_us": 2.46,
  "one_way_msgs_per_sec": 812347.5,
  "round_trip_msgs_per_sec": 406173.75
}
```

## 🎛️ **Configuration**

### **Command Line Options**
```bash
python dashboard.py [OPTIONS]

Options:
  --dir PATH          Data directory (required)
  --host HOST         Bind address (default: 127.0.0.1)
  --port PORT         Port number (default: 8050)  
  --debug             Enable debug mode (default: False)
```

### **Interactive Controls**
- **Mechanism Filtering**: Multi-select IPC mechanisms
- **Message Size Filtering**: Select specific byte sizes
- **File Browser**: Navigate and select data directories
- **Analysis Presets**: Quick configuration templates
- **Real-time Updates**: Dynamic chart regeneration

## 🔧 **Technical Requirements**

### **Dependencies**
- Python 3.9+
- Dash 3.2.0+
- Plotly 5.24.1+  
- Pandas 2.2.3+
- NumPy 2.1.1+
- Scikit-learn (optional, for ML anomaly detection)

### **Browser Support**
- Chrome/Edge 90+
- Firefox 88+
- Safari 14+

## 🐛 **Troubleshooting**

### **Common Issues**

**Dashboard won't start:**
```bash
# Check Python version and dependencies
python --version && pip list | grep dash

# Verify port availability  
lsof -i :8050
```

**Performance issues:**
- Large datasets may require additional memory
- Use data filtering for initial analysis
- Check browser console for JavaScript errors

**Data not loading:**
- Verify JSON/CSV file formats
- Check directory permissions
- Review terminal logs for error messages

## 🤝 **Contributing**

The dashboard is part of the [Rusty-Comms](../../README.md) benchmarking suite. Contributions welcome for:

- Additional visualization types
- Enhanced statistical analysis  
- Performance optimizations
- New data format support

## 📊 **Example Workflow**

1. **Start**: `python dashboard.py --dir ./results`
2. **Browse**: Use interactive file browser to select data
3. **Filter**: Choose mechanisms and message sizes
4. **Analyze**: Review summary insights and comparisons
5. **Deep Dive**: Switch to time series for temporal analysis
6. **Configure**: Apply presets or custom visualization settings
7. **Export**: Capture charts and insights for reporting

## 🔗 **Related Projects**

- [Rusty-Comms Main](../../README.md) - IPC benchmarking suite
- [Contributing Guide](../../CONTRIBUTING.md) - Development guidelines

---

**Built with modern web technologies and optimized for analyzing millions of IPC performance measurements.**