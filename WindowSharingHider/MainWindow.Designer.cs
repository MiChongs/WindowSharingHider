
namespace WindowSharingHider
{
    partial class MainWindow
    {
        /// <summary>
        /// Required designer variable.
        /// </summary>
        private System.ComponentModel.IContainer components = null;

        /// <summary>
        /// Clean up any resources being used.
        /// </summary>
        /// <param name="disposing">true if managed resources should be disposed; otherwise, false.</param>
        protected override void Dispose(bool disposing)
        {
            if (disposing && (components != null))
            {
                components.Dispose();
            }
            base.Dispose(disposing);
        }

        #region Windows Form Designer generated code

        /// <summary>
        /// Required method for Designer support - do not modify
        /// the contents of this method with the code editor.
        /// </summary>
        private void InitializeComponent()
        {
            this.windowListCheckBox = new System.Windows.Forms.CheckedListBox();
            this.hideWeTypeCandidateCheckBox = new System.Windows.Forms.CheckBox();
            this.showSystemWindowsCheckBox = new System.Windows.Forms.CheckBox();
            this.statusStrip = new System.Windows.Forms.StatusStrip();
            this.statusLabel = new System.Windows.Forms.ToolStripStatusLabel();
            this.statusStrip.SuspendLayout();
            this.SuspendLayout();
            // 
            // windowListCheckBox
            // 
            this.windowListCheckBox.CheckOnClick = true;
            this.windowListCheckBox.Dock = System.Windows.Forms.DockStyle.Fill;
            this.windowListCheckBox.FormattingEnabled = true;
            this.windowListCheckBox.Location = new System.Drawing.Point(0, 48);
            this.windowListCheckBox.Name = "windowListCheckBox";
            this.windowListCheckBox.Size = new System.Drawing.Size(468, 338);
            this.windowListCheckBox.TabIndex = 0;
            // 
            // hideWeTypeCandidateCheckBox
            // 
            this.hideWeTypeCandidateCheckBox.Dock = System.Windows.Forms.DockStyle.Top;
            this.hideWeTypeCandidateCheckBox.Location = new System.Drawing.Point(0, 0);
            this.hideWeTypeCandidateCheckBox.Name = "hideWeTypeCandidateCheckBox";
            this.hideWeTypeCandidateCheckBox.Padding = new System.Windows.Forms.Padding(8, 0, 0, 0);
            this.hideWeTypeCandidateCheckBox.Size = new System.Drawing.Size(468, 24);
            this.hideWeTypeCandidateCheckBox.TabIndex = 1;
            this.hideWeTypeCandidateCheckBox.Text = "隐藏 微信输入法";
            this.hideWeTypeCandidateCheckBox.UseVisualStyleBackColor = true;
            // 
            // showSystemWindowsCheckBox
            // 
            this.showSystemWindowsCheckBox.Dock = System.Windows.Forms.DockStyle.Top;
            this.showSystemWindowsCheckBox.Location = new System.Drawing.Point(0, 24);
            this.showSystemWindowsCheckBox.Name = "showSystemWindowsCheckBox";
            this.showSystemWindowsCheckBox.Padding = new System.Windows.Forms.Padding(8, 0, 0, 0);
            this.showSystemWindowsCheckBox.Size = new System.Drawing.Size(468, 24);
            this.showSystemWindowsCheckBox.TabIndex = 2;
            this.showSystemWindowsCheckBox.Text = "Show system / IME windows";
            this.showSystemWindowsCheckBox.UseVisualStyleBackColor = true;
            // 
            // statusStrip
            // 
            this.statusStrip.Items.AddRange(new System.Windows.Forms.ToolStripItem[] {
            this.statusLabel});
            this.statusStrip.Location = new System.Drawing.Point(0, 362);
            this.statusStrip.Name = "statusStrip";
            this.statusStrip.Size = new System.Drawing.Size(468, 22);
            this.statusStrip.TabIndex = 2;
            this.statusStrip.Text = "statusStrip";
            // 
            // statusLabel
            // 
            this.statusLabel.Name = "statusLabel";
            this.statusLabel.Size = new System.Drawing.Size(39, 17);
            this.statusLabel.Text = "Ready";
            // 
            // MainWindow
            // 
            this.AutoScaleDimensions = new System.Drawing.SizeF(6F, 13F);
            this.AutoScaleMode = System.Windows.Forms.AutoScaleMode.Font;
            this.ClientSize = new System.Drawing.Size(468, 384);
            this.Controls.Add(this.windowListCheckBox);
            this.Controls.Add(this.showSystemWindowsCheckBox);
            this.Controls.Add(this.hideWeTypeCandidateCheckBox);
            this.Controls.Add(this.statusStrip);
            this.Name = "MainWindow";
            this.Text = "Window Sharing Hider";
            this.statusStrip.ResumeLayout(false);
            this.statusStrip.PerformLayout();
            this.ResumeLayout(false);
            this.PerformLayout();

        }

        #endregion

        private System.Windows.Forms.CheckedListBox windowListCheckBox;
        private System.Windows.Forms.CheckBox hideWeTypeCandidateCheckBox;
        private System.Windows.Forms.CheckBox showSystemWindowsCheckBox;
        private System.Windows.Forms.StatusStrip statusStrip;
        private System.Windows.Forms.ToolStripStatusLabel statusLabel;
    }
}
